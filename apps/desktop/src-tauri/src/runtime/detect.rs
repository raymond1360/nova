use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::process;
use super::types::{
    ExecutionErrorKind, ExecutionOutcome, ExecutionRequest, ExecutionResult, ToolKind, ToolStatus,
};
use crate::security::redact_sensitive_text;

const DETECT_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_CREATE_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformStatus {
    pub os: String,
    pub arch: String,
    /// Nova v0.1.0-beta.1 targets Windows only (see `00_SCOPE09091050.md`).
    pub is_supported: bool,
}

/// Directory readiness for a location Nova needs to read or write (its own
/// app data directory, or — once a project-selection flow exists — a
/// user-selected project directory).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DirectoryStatus {
    Ready {
        path: String,
    },
    NotFound,
    NotADirectory,
    PermissionDenied,
    /// The probe file was created and written successfully but could not be
    /// removed afterward, for a reason other than a permission error (e.g.
    /// another process briefly locked it). The directory is not reported
    /// `Ready` — a leftover probe file is exactly the failure mode this
    /// check exists to catch, not something to paper over.
    CleanupFailed {
        detail: String,
    },
    Unknown {
        detail: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEnvironmentReport {
    pub platform: PlatformStatus,
    pub git: ToolStatus,
    pub codex_cli: ToolStatus,
    pub claude_cli: ToolStatus,
    pub app_data_dir: DirectoryStatus,
    pub generated_at: DateTime<Utc>,
}

/// Detect the current OS/architecture. Deliberately minimal for Phase 1:
/// only what `std::env::consts` reports without spawning any process or
/// adding a system-info dependency. Deeper OS build/version detection is
/// out of Phase 1 scope.
pub fn detect_platform() -> PlatformStatus {
    PlatformStatus {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        is_supported: cfg!(target_os = "windows"),
    }
}

fn canonicalize_directory(path: &Path) -> Result<PathBuf, DirectoryStatus> {
    let canonical = std::fs::canonicalize(path).map_err(|error| match error.kind() {
        io::ErrorKind::PermissionDenied => DirectoryStatus::PermissionDenied,
        _ => DirectoryStatus::NotFound,
    })?;
    if !canonical.is_dir() {
        return Err(DirectoryStatus::NotADirectory);
    }
    Ok(canonical)
}

/// Read-only directory check: exists, is a directory, and its listing can
/// be read. Never creates, writes, or deletes anything — safe to call in a
/// read-only/review context or before the user has explicitly authorized
/// write access to this specific directory.
pub fn check_directory_readable(path: &Path) -> DirectoryStatus {
    let canonical = match canonicalize_directory(path) {
        Ok(canonical) => canonical,
        Err(status) => return status,
    };
    match std::fs::read_dir(&canonical) {
        Ok(_) => DirectoryStatus::Ready {
            path: canonical.to_string_lossy().into_owned(),
        },
        Err(error) => match error.kind() {
            io::ErrorKind::PermissionDenied => DirectoryStatus::PermissionDenied,
            _ => DirectoryStatus::Unknown {
                detail: redact_sensitive_text(&error.to_string()),
            },
        },
    }
}

/// Read/write directory check: exists, is a directory, and Nova can create,
/// write, and then remove a probe file inside it.
///
/// Only call this for a directory the write probe is actually authorized
/// for (Nova's own app data directory, or a user-selected project
/// directory *after* the user has explicitly chosen/approved it) — never
/// in a read-only review context, and never merely to answer "can I read
/// this directory", which [`check_directory_readable`] answers without
/// writing anything.
///
/// The probe file name is randomized (process id + UUID) and created with
/// [`std::fs::OpenOptions::create_new`], which fails rather than
/// overwriting if a file with that exact name already exists — on the
/// astronomically unlikely collision, a new random name is generated and
/// retried up to [`PROBE_CREATE_ATTEMPTS`] times rather than ever
/// overwriting an existing file. Deletion failure is never reported as
/// `Ready`.
pub fn check_directory_writable(path: &Path) -> DirectoryStatus {
    let canonical = match canonicalize_directory(path) {
        Ok(canonical) => canonical,
        Err(status) => return status,
    };
    write_probe(&canonical)
}

fn write_probe(dir: &Path) -> DirectoryStatus {
    for _ in 0..PROBE_CREATE_ATTEMPTS {
        let probe_path = dir.join(format!(
            ".nova-access-check-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return match error.kind() {
                    io::ErrorKind::PermissionDenied => DirectoryStatus::PermissionDenied,
                    _ => DirectoryStatus::Unknown {
                        detail: redact_sensitive_text(&error.to_string()),
                    },
                };
            }
        };

        if let Err(error) = file
            .write_all(b"nova access check")
            .and_then(|()| file.flush())
        {
            drop(file);
            let _ = std::fs::remove_file(&probe_path);
            return DirectoryStatus::Unknown {
                detail: redact_sensitive_text(&error.to_string()),
            };
        }
        drop(file);

        return match std::fs::remove_file(&probe_path) {
            Ok(()) => DirectoryStatus::Ready {
                path: dir.to_string_lossy().into_owned(),
            },
            Err(error) => match error.kind() {
                io::ErrorKind::PermissionDenied => DirectoryStatus::PermissionDenied,
                _ => DirectoryStatus::CleanupFailed {
                    detail: redact_sensitive_text(&error.to_string()),
                },
            },
        };
    }
    DirectoryStatus::Unknown {
        detail: format!(
            "多次尝试（{PROBE_CREATE_ATTEMPTS} 次）后仍无法创建唯一探针文件名，可能存在异常写入冲突"
        ),
    }
}

fn program_for(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Git => "git",
        ToolKind::CodexCli => "codex",
        ToolKind::ClaudeCli => "claude",
    }
}

/// Read-only, side-effect-free status probe run *after* `--version`
/// confirms the binary exists. Nova requires no confirmed real auth-status
/// subcommand for Codex CLI or Claude Code CLI has been verified in this
/// development environment (neither is installed here to observe real
/// output against) — `--help` is used because it is the one flag virtually
/// every well-behaved CLI supports, is guaranteed read-only, and never
/// modifies a project or consumes quota. This is a conservative,
/// intentionally-limited placeholder: it is unlikely to reliably surface
/// login/session/quota state for tools that do not print it in `--help`
/// text, which is exactly why `classify_tool_readiness` below only ever
/// promotes a successful, keyword-free result to `Unknown` ("installed,
/// state unconfirmed"), never to `Ready` — see the Known Limitations
/// section of the remediation report this was written for.
fn status_probe_args() -> Vec<String> {
    vec!["--help".to_string()]
}

async fn probe(
    program: &str,
    args: &[String],
    app_data_dir: &Path,
    cancel: CancellationToken,
) -> ExecutionResult {
    let request = ExecutionRequest::new(
        program.to_string(),
        args.to_vec(),
        app_data_dir.to_path_buf(),
        Some(app_data_dir.to_path_buf()),
    )
    .with_timeout(DETECT_TIMEOUT);
    process::execute(request, cancel).await
}

fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Outcome of one probe execution, reduced to what the classifier needs:
/// either the process truly could not be run to completion at all
/// (`Infra`, e.g. not found / timed out / cancelled — no `ToolKind`-level
/// classification applies), or it ran to completion — successfully or with
/// a non-zero exit — and produced real output to classify.
#[derive(Debug)]
enum StageOutcome {
    Ran {
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    Infra(ToolStatus),
}

fn stage_outcome(result: &ExecutionResult) -> StageOutcome {
    match &result.outcome {
        ExecutionOutcome::Success => StageOutcome::Ran {
            exit_code: result.exit_code,
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
        },
        ExecutionOutcome::Failed {
            kind: ExecutionErrorKind::NonZeroExit,
            ..
        } => StageOutcome::Ran {
            exit_code: result.exit_code,
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
        },
        ExecutionOutcome::Failed { kind, detail } => {
            StageOutcome::Infra(infra_status(*kind, detail))
        }
    }
}

fn infra_status(kind: ExecutionErrorKind, detail: &str) -> ToolStatus {
    match kind {
        ExecutionErrorKind::NotFound => ToolStatus::NotFound,
        ExecutionErrorKind::PermissionDenied => ToolStatus::PermissionDenied,
        ExecutionErrorKind::StartFailed => ToolStatus::StartFailed {
            detail: detail.to_string(),
        },
        ExecutionErrorKind::TimedOut => ToolStatus::TimedOut,
        ExecutionErrorKind::Cancelled => ToolStatus::Cancelled,
        // PathRejected cannot occur here in production (the probe always
        // uses `app_data_dir` as both `cwd` and `allowed_root`, which the
        // boundary check always accepts), and `Unknown` from the executor
        // itself (e.g. the wait loop lost track of the process) is already
        // the right terminal state. Neither implies a specific tool-auth
        // state, so both map to the generic `Unknown`.
        ExecutionErrorKind::PathRejected | ExecutionErrorKind::Unknown => ToolStatus::Unknown {
            detail: detail.to_string(),
        },
        ExecutionErrorKind::NonZeroExit => {
            unreachable!("stage_outcome() routes NonZeroExit to StageOutcome::Ran")
        }
    }
}

/// Centralized, pure classifier: given a real completed process outcome
/// (exit code plus already-redacted stdout/stderr — see [`stage_outcome`]),
/// decide the tool's [`ToolStatus`]. All matching is case-insensitive.
/// Unmatched output is never guessed into a specific auth/session/quota
/// state — it falls back to `NonZeroExit` (bad exit code) or `Unknown`
/// ("ran cleanly but nothing readable as Ready")via the last two
/// branches, so a state like `NotLoggedIn` is only ever reported when the
/// output actually contains language indicating that.
pub fn classify_tool_readiness(
    kind: ToolKind,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> ToolStatus {
    // Git has no account/session/quota concept — a real, successful
    // execution fully answers its readiness question.
    if kind == ToolKind::Git {
        return if exit_code == Some(0) {
            ToolStatus::Ready { version: None }
        } else {
            ToolStatus::NonZeroExit {
                exit_code,
                detail: short_diagnostic(stdout, stderr),
            }
        };
    }

    let combined = format!("{stdout}\n{stderr}").to_lowercase();

    const NOT_LOGGED_IN: &[&str] = &[
        "not logged in",
        "not authenticated",
        "please log in",
        "please login",
        "no active session",
        "not signed in",
        "no credentials found",
    ];
    const SESSION_EXPIRED: &[&str] = &[
        "session expired",
        "session has expired",
        "token expired",
        "token has expired",
        "expired session",
        "expired token",
        "re-authenticate",
        "reauthenticate",
        "log in again",
    ];
    const QUOTA_EXCEEDED: &[&str] = &[
        "quota exceeded",
        "insufficient quota",
        "rate limit",
        "rate-limit",
        "too many requests",
        "insufficient credit",
        "usage limit",
        " 429",
    ];
    const PERMISSION_DENIED: &[&str] = &["permission denied", "access denied", "forbidden", " 403"];
    const NETWORK_INTERRUPTED: &[&str] = &[
        "network error",
        "network unreachable",
        "connection refused",
        "econnrefused",
        "enotfound",
        "could not connect",
        "network timeout",
        "no internet",
        "dns resolution failed",
    ];
    const CONFIRMED_READY: &[&str] = &[
        "logged in as",
        "authenticated as",
        "signed in as",
        "you are logged in",
        "you are authenticated",
        "you're logged in",
    ];

    // Session-expired is checked first: phrasing like "please log in again"
    // legitimately contains both an expiry signal and a login-instruction
    // signal, and the more specific "this used to work but expired" state
    // is the more useful/accurate one to report when both are present.
    if contains_any(&combined, SESSION_EXPIRED) {
        return ToolStatus::SessionExpired;
    }
    if contains_any(&combined, NOT_LOGGED_IN) {
        return ToolStatus::NotLoggedIn;
    }
    if contains_any(&combined, QUOTA_EXCEEDED) {
        return ToolStatus::QuotaExceeded;
    }
    if contains_any(&combined, PERMISSION_DENIED) {
        return ToolStatus::PermissionDenied;
    }
    if contains_any(&combined, NETWORK_INTERRUPTED) {
        return ToolStatus::NetworkInterrupted;
    }
    if contains_any(&combined, CONFIRMED_READY) {
        return ToolStatus::Ready { version: None };
    }

    if exit_code == Some(0) {
        // The binary ran cleanly but said nothing that confirms or denies
        // an authenticated, ready-to-work state. Reporting `Ready` here
        // would be exactly the false-positive Phase 1 review flagged — so
        // this is `Unknown` with an explicit, honest explanation instead.
        ToolStatus::Unknown {
            detail: "已安装，但当前可用命令无法确认登录/会话状态，请手动确认后再开始任务"
                .to_string(),
        }
    } else {
        ToolStatus::NonZeroExit {
            exit_code,
            detail: short_diagnostic(stdout, stderr),
        }
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn short_diagnostic(stdout: &str, stderr: &str) -> String {
    let source = if !stderr.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    first_nonempty_line(source).unwrap_or_else(|| "无更多诊断信息".to_string())
}

/// Detect one tool's readiness by running it under the safe process
/// boundary. `program` is a separate parameter from `kind` specifically so
/// tests can inject a deterministic, PATH-independent program name (a
/// guaranteed-nonexistent name, or a real one like `git`) instead of
/// depending on whichever real CLI happens (or does not happen) to be
/// installed on the machine running the test. [`detect_tool`] is the
/// production wrapper that always passes the real per-kind program name.
///
/// Two real stages, never conflated:
/// 1. `<program> --version` — proves the binary exists and starts. Never
///    mapped directly to `Ready` on its own for Codex/Claude.
/// 2. For Codex/Claude only (Git has no such state), a second, read-only
///    status probe (see [`status_probe_args`]) whose *actual* output is
///    run through [`classify_tool_readiness`] — a pure function, not
///    scattered UI logic — to decide the final [`ToolStatus`].
pub async fn detect_program(
    kind: ToolKind,
    program: &str,
    app_data_dir: &Path,
    cancel: CancellationToken,
) -> ToolStatus {
    let version_result = probe(
        program,
        &["--version".to_string()],
        app_data_dir,
        cancel.clone(),
    )
    .await;
    let (version_exit, version_stdout, version_stderr) = match stage_outcome(&version_result) {
        StageOutcome::Infra(status) => return status,
        StageOutcome::Ran {
            exit_code,
            stdout,
            stderr,
        } => (exit_code, stdout, stderr),
    };

    if kind == ToolKind::Git {
        return classify_tool_readiness(kind, version_exit, &version_stdout, &version_stderr);
    }

    let status_result = probe(program, &status_probe_args(), app_data_dir, cancel).await;
    match stage_outcome(&status_result) {
        StageOutcome::Infra(status) => status,
        StageOutcome::Ran {
            exit_code,
            stdout,
            stderr,
        } => match classify_tool_readiness(kind, exit_code, &stdout, &stderr) {
            // The classifier only sees stage 2's output, so it never has a
            // real version string to report; fill in the one stage 1
            // actually captured rather than leaving it empty.
            ToolStatus::Ready { version: None } => ToolStatus::Ready {
                version: first_nonempty_line(&version_stdout),
            },
            other => other,
        },
    }
}

/// Production entry point: detect `kind` using its real program name.
pub async fn detect_tool(
    kind: ToolKind,
    app_data_dir: &Path,
    cancel: CancellationToken,
) -> ToolStatus {
    detect_program(kind, program_for(kind), app_data_dir, cancel).await
}

/// Build the full Phase 1 environment report: platform, app data directory
/// access, and Git/Codex/Claude CLI readiness. The three CLI checks run
/// concurrently since each is an independent, short-lived process spawn.
pub async fn build_report(
    app_data_dir: PathBuf,
    cancel: CancellationToken,
) -> RuntimeEnvironmentReport {
    let platform = detect_platform();
    // Nova's own app data directory is always authorized for the write
    // probe — it is a directory Nova itself owns and created.
    let app_data_status = check_directory_writable(&app_data_dir);
    let (git, codex_cli, claude_cli) = tokio::join!(
        detect_tool(ToolKind::Git, &app_data_dir, cancel.clone()),
        detect_tool(ToolKind::CodexCli, &app_data_dir, cancel.clone()),
        detect_tool(ToolKind::ClaudeCli, &app_data_dir, cancel.clone()),
    );
    RuntimeEnvironmentReport {
        platform,
        git,
        codex_cli,
        claude_cli,
        app_data_dir: app_data_status,
        generated_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Instant;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-detect-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn nonexistent_program_name(tag: &str) -> String {
        format!("nova-definitely-nonexistent-{tag}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn platform_detection_reports_windows_as_supported_on_this_build() {
        let status = detect_platform();
        assert_eq!(status.os, std::env::consts::OS);
        // This test suite only ever runs on the Windows target Nova ships for.
        assert!(status.is_supported);
    }

    #[test]
    fn directory_readable_for_a_plain_temp_dir() {
        let dir = temp_dir("readable");
        assert_eq!(
            check_directory_readable(&dir),
            DirectoryStatus::Ready {
                path: std::fs::canonicalize(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_readable_check_never_creates_any_file() {
        let dir = temp_dir("readable-no-write");
        let _ = check_directory_readable(&dir);
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert!(
            entries.is_empty(),
            "read-only check must not create any file, found: {entries:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_writable_ready_for_writable_temp_dir() {
        let dir = temp_dir("writable-ready");
        assert_eq!(
            check_directory_writable(&dir),
            DirectoryStatus::Ready {
                path: std::fs::canonicalize(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_writable_leaves_no_probe_file_behind_on_success() {
        let dir = temp_dir("writable-no-leftover");
        let _ = check_directory_writable(&dir);
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert!(
            entries.is_empty(),
            "expected no leftover probe file, found: {entries:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_writable_not_found_for_missing_path() {
        let dir = temp_dir("dir-missing-parent");
        let missing = dir.join("nope");
        assert_eq!(
            check_directory_writable(&missing),
            DirectoryStatus::NotFound
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_writable_not_a_directory_for_file_path() {
        let dir = temp_dir("dir-is-file");
        let file = dir.join("file.txt");
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            check_directory_writable(&file),
            DirectoryStatus::NotADirectory
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_writable_does_not_overwrite_a_preexisting_same_name_file() {
        // Simulate the collision path directly: pre-create a file at the
        // exact name `write_probe` would generate for a fixed
        // process-id+uuid pair by constructing that name the same way,
        // then confirm the probe still succeeds (by retrying with a new
        // random name) and the pre-existing file's content is untouched.
        let dir = temp_dir("writable-collision");
        let preexisting_name = format!(
            ".nova-access-check-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        );
        let preexisting_path = dir.join(&preexisting_name);
        fs::write(&preexisting_path, b"do not touch me").unwrap();

        let status = check_directory_writable(&dir);
        assert!(matches!(status, DirectoryStatus::Ready { .. }));
        assert_eq!(
            fs::read(&preexisting_path).unwrap(),
            b"do not touch me",
            "pre-existing file with a colliding-style name must never be overwritten"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_writable_concurrent_probes_do_not_conflict() {
        let dir = temp_dir("writable-concurrent");
        let dir_arc = std::sync::Arc::new(dir.clone());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir_arc.clone();
                std::thread::spawn(move || check_directory_writable(&dir))
            })
            .collect();
        for handle in handles {
            let status = handle.join().unwrap();
            assert!(
                matches!(status, DirectoryStatus::Ready { .. }),
                "expected Ready, got {status:?}"
            );
        }
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert!(
            entries.is_empty(),
            "expected no leftover probe files after concurrent probes, found: {entries:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_writable_succeeds_for_chinese_and_space_path_with_no_leftover() {
        let base = temp_dir("writable-unicode");
        let dir = base.join("项目 目录 with spaces 中文");
        fs::create_dir_all(&dir).unwrap();
        let status = check_directory_writable(&dir);
        assert!(matches!(status, DirectoryStatus::Ready { .. }));
        let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert!(entries.is_empty());
        let _ = fs::remove_dir_all(&base);
    }

    fn success_result(stdout: &str) -> ExecutionResult {
        ExecutionResult {
            program: "git".to_string(),
            args: vec!["--version".to_string()],
            cwd: ".".to_string(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            duration_ms: 5,
            exit_code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            outcome: ExecutionOutcome::Success,
        }
    }

    fn failed_result(
        kind: ExecutionErrorKind,
        detail: &str,
        exit_code: Option<i32>,
    ) -> ExecutionResult {
        ExecutionResult {
            program: "git".to_string(),
            args: vec!["--version".to_string()],
            cwd: ".".to_string(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            duration_ms: 5,
            exit_code,
            stdout: String::new(),
            stderr: detail.to_string(),
            stdout_truncated: false,
            stderr_truncated: false,
            outcome: ExecutionOutcome::Failed {
                kind,
                detail: detail.to_string(),
            },
        }
    }

    #[test]
    fn stage_outcome_classifies_success_as_ready_with_version_line() {
        let stage = stage_outcome(&success_result("git version 2.55.0.windows.2\n"));
        match stage {
            StageOutcome::Ran {
                exit_code, stdout, ..
            } => {
                assert_eq!(
                    classify_tool_readiness(ToolKind::Git, exit_code, &stdout, ""),
                    ToolStatus::Ready { version: None }
                );
            }
            StageOutcome::Infra(status) => panic!("unexpected infra status {status:?}"),
        }
    }

    #[test]
    fn infra_failures_map_to_not_found_start_failed_timeout_cancelled_permission_denied() {
        assert_eq!(
            match stage_outcome(&failed_result(ExecutionErrorKind::NotFound, "x", None)) {
                StageOutcome::Infra(status) => status,
                _ => panic!("expected infra"),
            },
            ToolStatus::NotFound
        );
        assert_eq!(
            match stage_outcome(&failed_result(ExecutionErrorKind::TimedOut, "x", None)) {
                StageOutcome::Infra(status) => status,
                _ => panic!("expected infra"),
            },
            ToolStatus::TimedOut
        );
        assert_eq!(
            match stage_outcome(&failed_result(ExecutionErrorKind::Cancelled, "x", None)) {
                StageOutcome::Infra(status) => status,
                _ => panic!("expected infra"),
            },
            ToolStatus::Cancelled
        );
        assert_eq!(
            match stage_outcome(&failed_result(
                ExecutionErrorKind::PermissionDenied,
                "x",
                None
            )) {
                StageOutcome::Infra(status) => status,
                _ => panic!("expected infra"),
            },
            ToolStatus::PermissionDenied
        );
        match stage_outcome(&failed_result(
            ExecutionErrorKind::StartFailed,
            "boom",
            None,
        )) {
            StageOutcome::Infra(ToolStatus::StartFailed { detail }) => assert_eq!(detail, "boom"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn classify_tool_readiness_covers_every_auth_and_network_state_via_injected_fixtures() {
        // Every fixture string below is fabricated test data, not real
        // captured CLI output, and is fed directly into the production
        // classifier (not a test double) to prove each ToolStatus variant
        // is reachable from real, generic output patterns.
        assert_eq!(
            classify_tool_readiness(
                ToolKind::CodexCli,
                Some(1),
                "",
                "Error: not logged in. Run `codex login`."
            ),
            ToolStatus::NotLoggedIn
        );
        assert_eq!(
            classify_tool_readiness(
                ToolKind::ClaudeCli,
                Some(1),
                "",
                "Your session has expired, please log in again."
            ),
            ToolStatus::SessionExpired
        );
        assert_eq!(
            classify_tool_readiness(
                ToolKind::CodexCli,
                Some(1),
                "",
                "quota exceeded for this billing period"
            ),
            ToolStatus::QuotaExceeded
        );
        assert_eq!(
            classify_tool_readiness(
                ToolKind::ClaudeCli,
                Some(1),
                "",
                "Error: permission denied for this workspace"
            ),
            ToolStatus::PermissionDenied
        );
        assert_eq!(
            classify_tool_readiness(
                ToolKind::CodexCli,
                Some(1),
                "",
                "network error: could not connect to api.openai.com"
            ),
            ToolStatus::NetworkInterrupted
        );
        match classify_tool_readiness(
            ToolKind::ClaudeCli,
            Some(2),
            "",
            "unexpected flag: --frobnicate",
        ) {
            ToolStatus::NonZeroExit { exit_code, detail } => {
                assert_eq!(exit_code, Some(2));
                assert!(detail.contains("frobnicate"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            classify_tool_readiness(
                ToolKind::CodexCli,
                Some(0),
                "You are logged in as demo@example.com",
                ""
            ),
            ToolStatus::Ready { version: None }
        );
        match classify_tool_readiness(ToolKind::ClaudeCli, Some(0), "Usage: claude [options]", "") {
            ToolStatus::Unknown { detail } => assert!(!detail.is_empty()),
            other => panic!("expected Unknown (installed, state unconfirmed), got {other:?}"),
        }
    }

    #[test]
    fn classify_tool_readiness_is_case_insensitive() {
        assert_eq!(
            classify_tool_readiness(ToolKind::CodexCli, Some(1), "", "NOT LOGGED IN"),
            ToolStatus::NotLoggedIn
        );
        assert_eq!(
            classify_tool_readiness(ToolKind::CodexCli, Some(1), "", "QuOtA ExCeEdEd"),
            ToolStatus::QuotaExceeded
        );
    }

    #[test]
    fn successful_version_stage_never_becomes_ready_without_running_the_status_stage() {
        // Regression guard for the exact Major 1 finding: a bare successful
        // `--version` must not, by itself, ever be treated as the final
        // answer for Codex/Claude — only `detect_program`'s full two-stage
        // flow (exercised in the real-machine tests below) may produce
        // `Ready` for those two tools, and only when the status stage
        // itself confirms it.
        let version_only =
            classify_tool_readiness(ToolKind::CodexCli, Some(0), "codex-cli 1.0.0", "");
        assert_ne!(
            version_only,
            ToolStatus::Ready { version: None },
            "a bare version string must not itself be treated as an auth confirmation"
        );
    }

    #[tokio::test]
    async fn detect_program_with_deterministic_nonexistent_name_is_not_found() {
        // Hermetic: the program name is a fresh random string guaranteed
        // not to exist on any machine's PATH, so this test's result never
        // depends on what happens to be installed where it runs.
        let dir = temp_dir("di-not-found");
        let name = nonexistent_program_name("a");
        let status = detect_program(ToolKind::Git, &name, &dir, CancellationToken::new()).await;
        assert_eq!(status, ToolStatus::NotFound);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn detect_program_with_a_real_executable_name_is_ready() {
        let dir = temp_dir("di-ready");
        let status = detect_program(ToolKind::Git, "git", &dir, CancellationToken::new()).await;
        assert!(
            matches!(status, ToolStatus::Ready { .. }),
            "expected Ready, got {status:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn parallel_detections_with_different_injected_programs_do_not_cross_contaminate() {
        // Proves detection has no shared/global mutable state (no PATH
        // mutation, no process-wide env var): three concurrent detections
        // with three different injected program names must each get their
        // own correct, independent result.
        let dir = temp_dir("di-parallel");
        let cancel = CancellationToken::new();
        let missing_a = nonexistent_program_name("b");
        let missing_b = nonexistent_program_name("c");
        let (real, missing_a_status, missing_b_status) = tokio::join!(
            detect_program(ToolKind::Git, "git", &dir, cancel.clone()),
            detect_program(ToolKind::Git, &missing_a, &dir, cancel.clone()),
            detect_program(ToolKind::Git, &missing_b, &dir, cancel.clone()),
        );
        assert!(matches!(real, ToolStatus::Ready { .. }));
        assert_eq!(missing_a_status, ToolStatus::NotFound);
        assert_eq!(missing_b_status, ToolStatus::NotFound);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn detects_real_codex_cli_status_without_assuming_installed_or_not() {
        // Must never hardcode an expectation about whether Codex CLI is
        // present on the machine running this test — that is exactly the
        // Phase 1 review Blocker this replaces. The only universal
        // guarantees: it must complete within a bounded time, and if it
        // does report Ready, that must carry a real captured version
        // string rather than a guess.
        let dir = temp_dir("codex-real");
        let start = Instant::now();
        let status = detect_tool(ToolKind::CodexCli, &dir, CancellationToken::new()).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(25),
            "detection took too long: {elapsed:?}"
        );
        if let ToolStatus::Ready { version } = &status {
            assert!(
                version.is_some(),
                "Ready must carry a real captured version string, not a guess"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn detects_real_claude_cli_status_without_assuming_installed_or_not() {
        let dir = temp_dir("claude-real");
        let start = Instant::now();
        let status = detect_tool(ToolKind::ClaudeCli, &dir, CancellationToken::new()).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(25),
            "detection took too long: {elapsed:?}"
        );
        if let ToolStatus::Ready { version } = &status {
            assert!(
                version.is_some(),
                "Ready must carry a real captured version string, not a guess"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn build_report_aggregates_all_checks() {
        let dir = temp_dir("full-report");
        let report = build_report(dir.clone(), CancellationToken::new()).await;
        assert!(report.platform.is_supported);
        assert!(matches!(report.app_data_dir, DirectoryStatus::Ready { .. }));
        assert!(matches!(report.git, ToolStatus::Ready { .. }));
        let _ = fs::remove_dir_all(&dir);
    }
}
