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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformStatus {
    pub os: String,
    pub arch: String,
    /// Nova v0.1.0-beta.1 targets Windows only (see `00_SCOPE09091050.md`).
    pub is_supported: bool,
}

/// Directory readiness for a location Nova needs to read/write (its own app
/// data directory, or a user-selected project directory).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DirectoryStatus {
    Ready { path: String },
    NotFound,
    NotADirectory,
    PermissionDenied,
    Unknown { detail: String },
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

/// Check that `path` exists, is a directory, and Nova can both read and
/// write inside it (via a small probe file that is created then removed).
/// Used for both Nova's own app data directory and a user-selected project
/// directory — the same access question applies to both.
pub fn check_directory_access(path: &Path) -> DirectoryStatus {
    let canonical = match std::fs::canonicalize(path) {
        Ok(canonical) => canonical,
        Err(error) => {
            return match error.kind() {
                std::io::ErrorKind::PermissionDenied => DirectoryStatus::PermissionDenied,
                _ => DirectoryStatus::NotFound,
            };
        }
    };
    if !canonical.is_dir() {
        return DirectoryStatus::NotADirectory;
    }
    let probe = canonical.join(format!(".nova-access-check-{}", uuid::Uuid::new_v4()));
    match std::fs::write(&probe, b"nova access check") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            DirectoryStatus::Ready {
                path: canonical.to_string_lossy().into_owned(),
            }
        }
        Err(error) => match error.kind() {
            std::io::ErrorKind::PermissionDenied => DirectoryStatus::PermissionDenied,
            _ => DirectoryStatus::Unknown {
                detail: redact_sensitive_text(&error.to_string()),
            },
        },
    }
}

fn program_for(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Git => "git",
        ToolKind::CodexCli => "codex",
        ToolKind::ClaudeCli => "claude",
    }
}

/// Detect one CLI tool's readiness by actually spawning it with `--version`
/// under the safe process boundary, from Nova's own app data directory
/// (never a user project directory — detection must not depend on one
/// being selected yet). Never trusts `--version` alone as full proof of
/// usability: it only ever yields `Ready`, and every other branch below is
/// reached from a real classified process outcome (spawn error kind, exit
/// code, timeout, or cancellation), not a guess.
pub async fn detect_tool(
    kind: ToolKind,
    app_data_dir: &Path,
    cancel: CancellationToken,
) -> ToolStatus {
    let request = ExecutionRequest::new(
        program_for(kind),
        vec!["--version".to_string()],
        app_data_dir.to_path_buf(),
        Some(app_data_dir.to_path_buf()),
    )
    .with_timeout(DETECT_TIMEOUT);
    let result = process::execute(request, cancel).await;
    classify_tool_status(&result)
}

fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn classify_tool_status(result: &ExecutionResult) -> ToolStatus {
    match &result.outcome {
        ExecutionOutcome::Success => ToolStatus::Ready {
            version: first_nonempty_line(&result.stdout),
        },
        ExecutionOutcome::Failed { kind, detail } => match kind {
            ExecutionErrorKind::NotFound => ToolStatus::NotFound,
            ExecutionErrorKind::PermissionDenied => ToolStatus::PermissionDenied,
            ExecutionErrorKind::StartFailed => ToolStatus::StartFailed {
                detail: detail.clone(),
            },
            ExecutionErrorKind::TimedOut => ToolStatus::TimedOut,
            ExecutionErrorKind::Cancelled => ToolStatus::Cancelled,
            ExecutionErrorKind::NonZeroExit => ToolStatus::NonZeroExit {
                exit_code: result.exit_code,
                detail: first_nonempty_line(&result.stderr).unwrap_or_else(|| detail.clone()),
            },
            // `PathRejected` cannot occur here: `app_data_dir` is passed as
            // both `cwd` and `allowed_root`, which the boundary check always
            // accepts (a directory is always inside itself). Kept mapped to
            // `Unknown` defensively rather than unreachable!(), since a
            // future refactor could change that invariant.
            ExecutionErrorKind::PathRejected | ExecutionErrorKind::Unknown => ToolStatus::Unknown {
                detail: detail.clone(),
            },
        },
    }
}

/// Build the full Phase 1 environment report: platform, app data directory
/// access, and Git/Codex/Claude CLI readiness. The three CLI checks run
/// concurrently since each is an independent, short-lived process spawn.
pub async fn build_report(
    app_data_dir: PathBuf,
    cancel: CancellationToken,
) -> RuntimeEnvironmentReport {
    let platform = detect_platform();
    let app_data_status = check_directory_access(&app_data_dir);
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

    #[test]
    fn platform_detection_reports_windows_as_supported_on_this_build() {
        let status = detect_platform();
        assert_eq!(status.os, std::env::consts::OS);
        // This test suite only ever runs on the Windows target Nova ships for.
        assert!(status.is_supported);
    }

    #[test]
    fn directory_access_ready_for_writable_temp_dir() {
        let dir = temp_dir("dir-ready");
        assert_eq!(
            check_directory_access(&dir),
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
    fn directory_access_not_found_for_missing_path() {
        let dir = temp_dir("dir-missing-parent");
        let missing = dir.join("nope");
        assert_eq!(check_directory_access(&missing), DirectoryStatus::NotFound);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_access_not_a_directory_for_file_path() {
        let dir = temp_dir("dir-is-file");
        let file = dir.join("file.txt");
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            check_directory_access(&file),
            DirectoryStatus::NotADirectory
        );
        let _ = fs::remove_dir_all(&dir);
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
    fn classifies_success_as_ready_with_version_line() {
        let status = classify_tool_status(&success_result("git version 2.55.0.windows.2\n"));
        assert_eq!(
            status,
            ToolStatus::Ready {
                version: Some("git version 2.55.0.windows.2".to_string())
            }
        );
    }

    #[test]
    fn classifies_not_found_start_failed_timeout_and_cancelled() {
        assert_eq!(
            classify_tool_status(&failed_result(ExecutionErrorKind::NotFound, "x", None)),
            ToolStatus::NotFound
        );
        assert_eq!(
            classify_tool_status(&failed_result(ExecutionErrorKind::TimedOut, "x", None)),
            ToolStatus::TimedOut
        );
        assert_eq!(
            classify_tool_status(&failed_result(ExecutionErrorKind::Cancelled, "x", None)),
            ToolStatus::Cancelled
        );
        assert_eq!(
            classify_tool_status(&failed_result(
                ExecutionErrorKind::PermissionDenied,
                "x",
                None
            )),
            ToolStatus::PermissionDenied
        );
        match classify_tool_status(&failed_result(
            ExecutionErrorKind::StartFailed,
            "boom",
            None,
        )) {
            ToolStatus::StartFailed { detail } => assert_eq!(detail, "boom"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn classifies_nonzero_exit_with_exit_code_and_stderr_detail() {
        let result = failed_result(ExecutionErrorKind::NonZeroExit, "usage: git ...", Some(129));
        match classify_tool_status(&result) {
            ToolStatus::NonZeroExit { exit_code, detail } => {
                assert_eq!(exit_code, Some(129));
                assert_eq!(detail, "usage: git ...");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn detects_installed_git_as_ready_for_real() {
        let dir = temp_dir("git-ready");
        let status = detect_tool(ToolKind::Git, &dir, CancellationToken::new()).await;
        assert!(
            matches!(status, ToolStatus::Ready { .. }),
            "expected git to be Ready, got {status:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn detects_codex_cli_as_not_found_when_absent_from_path() {
        // Reflects this development machine's real PATH state (verified via
        // `Get-Command codex` before writing this detector): no `codex`
        // executable is on PATH, so this must be a real NotFound, not a
        // simulated one.
        let dir = temp_dir("codex-missing");
        let status = detect_tool(ToolKind::CodexCli, &dir, CancellationToken::new()).await;
        assert_eq!(status, ToolStatus::NotFound);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn detects_claude_cli_as_not_found_when_absent_from_path() {
        let dir = temp_dir("claude-missing");
        let status = detect_tool(ToolKind::ClaudeCli, &dir, CancellationToken::new()).await;
        assert_eq!(status, ToolStatus::NotFound);
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
