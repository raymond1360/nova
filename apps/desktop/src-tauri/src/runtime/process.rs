use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use super::path_guard::{self, PathRejection};
use super::types::{ExecutionErrorKind, ExecutionOutcome, ExecutionRequest, ExecutionResult};
use crate::security::redact_sensitive_text;

const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Run one external program under Nova's safety boundary.
///
/// Contract enforced here, not left to callers:
/// - `program` + `args` are handed to the OS as an argument array
///   ([`Command::new`] + [`Command::args`]); nothing is ever concatenated
///   into a shell command string, and no `cmd.exe`/`powershell`/`sh -c`
///   wrapper is used.
/// - `cwd` must resolve, via [`path_guard::validate_directory_boundary`],
///   inside `allowed_root`. A request with no `allowed_root` is rejected
///   outright rather than silently allowed to run anywhere.
/// - stdout/stderr are captured separately, size-capped, and passed through
///   [`redact_sensitive_text`] before being stored.
/// - the child is always waited on (never left as a zombie/orphan): on
///   timeout or cancellation it is killed and then reaped.
pub async fn execute(request: ExecutionRequest, cancel: CancellationToken) -> ExecutionResult {
    tokio::task::spawn_blocking(move || execute_blocking(request, cancel))
        .await
        .unwrap_or_else(|join_error| {
            let now = Utc::now();
            ExecutionResult {
                program: String::new(),
                args: Vec::new(),
                cwd: String::new(),
                started_at: now,
                ended_at: now,
                duration_ms: 0,
                exit_code: None,
                stdout: String::new(),
                stderr: redact_sensitive_text(&join_error.to_string()),
                stdout_truncated: false,
                stderr_truncated: false,
                outcome: ExecutionOutcome::Failed {
                    kind: ExecutionErrorKind::Unknown,
                    detail: "执行线程异常终止".to_string(),
                },
            }
        })
}

/// Synchronous core of [`execute`]. Safe to call directly from tests or from
/// inside a `spawn_blocking` closure.
pub fn execute_blocking(request: ExecutionRequest, cancel: CancellationToken) -> ExecutionResult {
    let started_at = Utc::now();
    let start_instant = Instant::now();

    let resolved_cwd = match &request.allowed_root {
        Some(root) => path_guard::validate_directory_boundary(&request.cwd, root),
        None => Err(PathRejection::OutsideAllowedRoot),
    };
    let resolved_cwd = match resolved_cwd {
        Ok(path) => path,
        Err(rejection) => {
            return finished_without_process(
                &request,
                started_at,
                start_instant,
                ExecutionErrorKind::PathRejected,
                rejection.detail().to_string(),
            );
        }
    };

    let mut command = Command::new(&request.program);
    command
        .args(&request.args)
        .current_dir(&resolved_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_no_window(&mut command);

    let mut child: Child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let kind = match error.kind() {
                std::io::ErrorKind::NotFound => ExecutionErrorKind::NotFound,
                std::io::ErrorKind::PermissionDenied => ExecutionErrorKind::PermissionDenied,
                _ => ExecutionErrorKind::StartFailed,
            };
            return finished_without_process(
                &request,
                started_at,
                start_instant,
                kind,
                redact_sensitive_text(&error.to_string()),
            );
        }
    };

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_reader = stdout_pipe.map(|pipe| spawn_capped_reader(pipe, request.max_output_bytes));
    let stderr_reader = stderr_pipe.map(|pipe| spawn_capped_reader(pipe, request.max_output_bytes));

    let mut forced: Option<ExecutionErrorKind> = None;
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if cancel.is_cancelled() {
                    let _ = child.kill();
                    let _ = child.wait();
                    forced = Some(ExecutionErrorKind::Cancelled);
                    break None;
                }
                if start_instant.elapsed() >= request.timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    forced = Some(ExecutionErrorKind::TimedOut);
                    break None;
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                forced = Some(ExecutionErrorKind::Unknown);
                break None;
            }
        }
    };

    let (stdout_bytes, stdout_truncated) = join_reader(stdout_reader);
    let (stderr_bytes, stderr_truncated) = join_reader(stderr_reader);
    let stdout = redact_sensitive_text(&String::from_utf8_lossy(&stdout_bytes));
    let stderr = redact_sensitive_text(&String::from_utf8_lossy(&stderr_bytes));

    let ended_at = Utc::now();
    let duration_ms = start_instant.elapsed().as_millis() as u64;
    let exit_code = exit_status.and_then(|status| status.code());

    let outcome = match forced {
        Some(kind) => ExecutionOutcome::Failed {
            kind,
            detail: forced_kind_detail(kind).to_string(),
        },
        None => match exit_status {
            Some(status) if status.success() => ExecutionOutcome::Success,
            Some(_) => ExecutionOutcome::Failed {
                kind: ExecutionErrorKind::NonZeroExit,
                detail: format!(
                    "退出码 {}",
                    exit_code.map_or("未知".to_string(), |c| c.to_string())
                ),
            },
            None => ExecutionOutcome::Failed {
                kind: ExecutionErrorKind::Unknown,
                detail: "无法获取进程退出状态".to_string(),
            },
        },
    };

    ExecutionResult {
        program: request.program,
        args: request.args,
        cwd: resolved_cwd.to_string_lossy().into_owned(),
        started_at,
        ended_at,
        duration_ms,
        exit_code,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
        outcome,
    }
}

fn finished_without_process(
    request: &ExecutionRequest,
    started_at: chrono::DateTime<Utc>,
    start_instant: Instant,
    kind: ExecutionErrorKind,
    detail: String,
) -> ExecutionResult {
    ExecutionResult {
        program: request.program.clone(),
        args: request.args.clone(),
        cwd: request.cwd.to_string_lossy().into_owned(),
        started_at,
        ended_at: Utc::now(),
        duration_ms: start_instant.elapsed().as_millis() as u64,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        outcome: ExecutionOutcome::Failed { kind, detail },
    }
}

fn forced_kind_detail(kind: ExecutionErrorKind) -> &'static str {
    match kind {
        ExecutionErrorKind::Cancelled => "已被用户取消",
        ExecutionErrorKind::TimedOut => "执行超时，已终止进程",
        ExecutionErrorKind::Unknown => "无法读取进程状态",
        _ => "执行失败",
    }
}

type ReaderHandle = thread::JoinHandle<(Vec<u8>, bool)>;

fn spawn_capped_reader<R>(mut reader: R, cap: usize) -> ReaderHandle
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut collected = Vec::with_capacity(cap.min(64 * 1024));
        let mut truncated = false;
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if collected.len() < cap {
                        let remaining = cap - collected.len();
                        let take = remaining.min(read);
                        collected.extend_from_slice(&buffer[..take]);
                        if take < read {
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
                Err(_) => break,
            }
        }
        (collected, truncated)
    })
}

fn join_reader(handle: Option<ReaderHandle>) -> (Vec<u8>, bool) {
    match handle {
        Some(handle) => handle.join().unwrap_or_else(|_| (Vec::new(), false)),
        None => (Vec::new(), false),
    }
}

#[cfg(windows)]
fn apply_no_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn apply_no_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn temp_root(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-process-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn request(program: &str, args: Vec<&str>, root: &Path) -> ExecutionRequest {
        ExecutionRequest::new(
            program.to_string(),
            args.into_iter().map(str::to_string).collect(),
            root.to_path_buf(),
            Some(root.to_path_buf()),
        )
    }

    #[test]
    fn rejects_execution_outside_allowed_root_without_spawning() {
        let root = temp_root("boundary");
        let outside = temp_root("boundary-outside");
        let mut req = request("git", vec!["--version"], &root);
        req.cwd = outside.clone();
        let cancel = CancellationToken::new();
        let result = execute_blocking(req, cancel);
        assert!(!matches!(result.outcome, ExecutionOutcome::Success));
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::PathRejected)
            }
            _ => panic!("expected Failed outcome"),
        }
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn missing_executable_is_reported_as_not_found() {
        let root = temp_root("not-found");
        let req = request("nova-definitely-not-a-real-executable-xyz", vec![], &root);
        let result = execute_blocking(req, CancellationToken::new());
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => assert_eq!(kind, ExecutionErrorKind::NotFound),
            _ => panic!("expected NotFound"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_version_succeeds_and_captures_stdout() {
        let root = temp_root("git-version");
        let req = request("git", vec!["--version"], &root);
        let result = execute_blocking(req, CancellationToken::new());
        assert!(
            matches!(result.outcome, ExecutionOutcome::Success),
            "outcome: {:?}",
            result.outcome
        );
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.to_lowercase().contains("git version"));
        assert!(result.stderr.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn nonzero_exit_is_captured_with_separated_stderr() {
        let root = temp_root("nonzero-exit");
        // git with an invalid subcommand exits non-zero and writes to stderr.
        let req = request("git", vec!["not-a-real-git-subcommand"], &root);
        let result = execute_blocking(req, CancellationToken::new());
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::NonZeroExit)
            }
            _ => panic!("expected NonZeroExit, got {:?}", result.outcome),
        }
        assert_ne!(result.exit_code, Some(0));
        assert!(!result.stderr.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn timeout_kills_the_process_and_reports_timed_out() {
        let root = temp_root("timeout");
        // `ping` is spawned directly (not via `cmd /C`) so the killed
        // process has no un-killed grandchild: killing the direct child is
        // enough to leave no orphan process behind.
        let mut req = request("ping", vec!["-n", "30", "127.0.0.1"], &root);
        req.timeout = Duration::from_millis(300);
        let start = Instant::now();
        let result = execute_blocking(req, CancellationToken::new());
        let elapsed = start.elapsed();
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => assert_eq!(kind, ExecutionErrorKind::TimedOut),
            _ => panic!("expected TimedOut, got {:?}", result.outcome),
        }
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout enforcement took too long: {elapsed:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn user_cancellation_kills_the_process_and_reports_cancelled() {
        let root = temp_root("cancel");
        // Spawned directly (see comment in the timeout test above) so
        // cancellation cannot leave an orphaned grandchild process.
        let mut req = request("ping", vec!["-n", "30", "127.0.0.1"], &root);
        req.timeout = Duration::from_secs(30);
        let cancel = CancellationToken::new();
        let cancel_for_thread = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            cancel_for_thread.cancel();
        });
        let start = Instant::now();
        let result = execute_blocking(req, cancel);
        let elapsed = start.elapsed();
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::Cancelled)
            }
            _ => panic!("expected Cancelled, got {:?}", result.outcome),
        }
        assert!(
            elapsed < Duration::from_secs(5),
            "cancellation took too long: {elapsed:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn output_over_cap_is_truncated_not_unbounded() {
        let root = temp_root("output-cap");
        // Print far more than the cap using cmd.exe's FOR loop instead of
        // an external tool, keeping the test hermetic.
        let mut req = request(
            "cmd",
            vec![
                "/C",
                "for /L %I in (1,1,2000) do @echo line-of-filler-text-%I",
            ],
            &root,
        );
        req.max_output_bytes = 512;
        let result = execute_blocking(req, CancellationToken::new());
        assert!(result.stdout.len() <= 512 + 4); // small slack for lossy UTF-8 boundary
        assert!(result.stdout_truncated);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn argument_containing_shell_metacharacters_is_not_reinterpreted() {
        // The injected string must arrive at git as a single, literal
        // argument. Proof: git's own error message echoes it back whole,
        // which is only possible if no shell (cmd.exe/sh) ever re-parsed
        // `&&`, `;`, `|`, or `>` out of it.
        let root = temp_root("injection");
        let injected = "phase1 && calc.exe; rm -rf / | echo pwned > out.txt";
        let req = request("git", vec![injected], &root);
        let result = execute_blocking(req, CancellationToken::new());
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::NonZeroExit)
            }
            _ => panic!("expected git to reject the literal bogus argument"),
        }
        assert!(
            result.stderr.contains(injected) || result.stdout.contains(injected),
            "expected the injected string to appear verbatim in git's own error output, stdout={:?} stderr={:?}",
            result.stdout,
            result.stderr
        );
        let _ = fs::remove_dir_all(&root);
    }
}
