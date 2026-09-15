use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use super::path_guard::{self, PathRejection};
use super::types::{ExecutionErrorKind, ExecutionOutcome, ExecutionRequest, ExecutionResult};
use crate::security::redact_sensitive_text;

#[cfg(windows)]
use super::job_object;

const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Platform-specific process-tree containment handle. On Windows this is a
/// live [`job_object::JobObject`] the child was assigned to (`None` only if
/// containment could not be established — see [`setup_containment`], which
/// never lets execution proceed in that case). On non-Windows targets this
/// carries no state: Nova ships Windows-only for v0.1.0, and killing only
/// the direct child is the same level of protection this codebase always
/// had there, not a regression.
#[cfg(windows)]
type TreeContainer = job_object::JobObject;
#[cfg(not(windows))]
type TreeContainer = ();

#[cfg(windows)]
fn setup_containment(child: &Child) -> io::Result<TreeContainer> {
    let job = job_object::JobObject::create()?;
    job.assign(child)?;
    Ok(job)
}

#[cfg(not(windows))]
fn setup_containment(_child: &Child) -> io::Result<TreeContainer> {
    Ok(())
}

/// Terminate the whole process tree started for this execution, then reap
/// the direct child so it is never left as a zombie/orphan from Nova's
/// point of view. On Windows this terminates the Job Object (killing the
/// direct child *and* any descendants it spawned); on other platforms it
/// falls back to killing just the direct child.
#[cfg(windows)]
fn terminate_tree(child: &mut Child, container: &TreeContainer) {
    let _ = container.terminate();
    let _ = child.wait();
}

#[cfg(not(windows))]
fn terminate_tree(child: &mut Child, _container: &TreeContainer) {
    let _ = child.kill();
    let _ = child.wait();
}

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

    // Establish process-tree containment *before* touching stdio or
    // waiting, so the window in which a not-yet-contained child could
    // spawn an uncontained grandchild is as small as possible. If
    // containment cannot be established, this execution must not proceed
    // pretending it has a safety boundary it does not have: the
    // already-started child is killed and reaped, and a structured
    // failure is returned instead.
    let container = match setup_containment(&child) {
        Ok(container) => container,
        Err(error) => {
            return handle_containment_failure(
                &mut child,
                &request,
                started_at,
                start_instant,
                error,
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
                    terminate_tree(&mut child, &container);
                    forced = Some(ExecutionErrorKind::Cancelled);
                    break None;
                }
                if start_instant.elapsed() >= request.timeout {
                    terminate_tree(&mut child, &container);
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
    // `container` (the Job Object on Windows) is no longer needed once the
    // child has exited or been terminated; dropping it here is harmless —
    // `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` only matters while member
    // processes could still be running, and by this point they are not.
    drop(container);

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
        // `program`/`args` are Nova-constructed today (e.g. `"git"`,
        // `"--version"`), but future phases may place less-trusted content
        // in `args` (task/command text). Redacting them here, through the
        // exact same policy as stdout/stderr, means `ExecutionResult` can
        // never become a secret-leak path regardless of what a future
        // caller puts in the request — defense in depth, not a statement
        // that today's callers are untrusted.
        program: redact_sensitive_text(&request.program),
        args: request
            .args
            .iter()
            .map(|arg| redact_sensitive_text(arg))
            .collect(),
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

/// Handle the case where process-tree containment could not be established
/// for an already-spawned child: kill and reap it immediately (never leave
/// an uncontained process running just because containment setup failed)
/// and return a structured `StartFailed` result rather than silently
/// proceeding without the safety boundary Nova claims to provide.
///
/// Extracted as its own function (rather than inlined at the one real call
/// site) specifically so a test can drive it with a synthetic
/// [`io::Error`] — deterministically forcing the real Win32
/// `AssignProcessToJobObject` call to fail is not reliable across Windows
/// versions/privilege levels, but this function *is* the exact production
/// code that runs when it does fail, so injecting the failure here proves
/// the real cleanup behavior, not a simulation of it.
fn handle_containment_failure(
    child: &mut Child,
    request: &ExecutionRequest,
    started_at: chrono::DateTime<Utc>,
    start_instant: Instant,
    error: io::Error,
) -> ExecutionResult {
    let _ = child.kill();
    let _ = child.wait();
    finished_without_process(
        request,
        started_at,
        start_instant,
        ExecutionErrorKind::StartFailed,
        format!(
            "无法建立进程隔离边界，已终止已启动的进程：{}",
            redact_sensitive_text(&error.to_string())
        ),
    )
}

fn finished_without_process(
    request: &ExecutionRequest,
    started_at: chrono::DateTime<Utc>,
    start_instant: Instant,
    kind: ExecutionErrorKind,
    detail: String,
) -> ExecutionResult {
    ExecutionResult {
        program: redact_sensitive_text(&request.program),
        args: request
            .args
            .iter()
            .map(|arg| redact_sensitive_text(arg))
            .collect(),
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

    #[test]
    fn program_and_args_are_redacted_in_the_result() {
        let root = temp_root("redact-args");
        let fake_token = ["sk", "abcdefghijklmnop"].join("-");
        let req = request(
            "nova-definitely-not-a-real-executable-xyz",
            vec![&fake_token],
            &root,
        );
        let result = execute_blocking(req, CancellationToken::new());
        assert!(!result.args.iter().any(|a| a.contains(&fake_token)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn containment_failure_kills_and_reaps_the_already_started_child() {
        // Deterministically forcing the real Win32 `AssignProcessToJobObject`
        // call to fail is not reliable across Windows versions/privilege
        // levels (nested-job support varies). Instead, this drives the exact
        // production cleanup function (`handle_containment_failure`) with a
        // synthetic error, proving the already-spawned child is genuinely
        // killed and reaped rather than left running — the actual safety
        // property the requirement cares about.
        let root = temp_root("containment-failure");
        let req = request("ping", vec!["-n", "30", "127.0.0.1"], &root);
        let resolved_cwd = path_guard::validate_directory_boundary(&req.cwd, &root).unwrap();
        let mut command = Command::new(&req.program);
        command
            .args(&req.args)
            .current_dir(&resolved_cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply_no_window(&mut command);
        let mut child = command
            .spawn()
            .expect("spawn ping for containment-failure test");
        let pid = child.id();

        let started_at = Utc::now();
        let start_instant = Instant::now();
        let synthetic_error = io::Error::other("simulated job assignment failure");
        let result = handle_containment_failure(
            &mut child,
            &req,
            started_at,
            start_instant,
            synthetic_error,
        );

        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::StartFailed)
            }
            other => panic!("expected StartFailed, got {other:?}"),
        }
        // `child.wait()` inside `handle_containment_failure` already reaped
        // it; a second `try_wait()` must report it as gone, not "still
        // running", proving no process was left behind.
        assert!(
            !matches!(child.try_wait(), Ok(None)),
            "process {pid} should have been killed and reaped on containment failure"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    fn process_is_running(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut exit_code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut exit_code);
            CloseHandle(handle);
            ok != 0 && exit_code == STILL_ACTIVE
        }
    }

    #[cfg(windows)]
    fn wait_until_not_running(pid: u32, attempts: u32) -> bool {
        let mut running = process_is_running(pid);
        for _ in 0..attempts {
            if !running {
                break;
            }
            thread::sleep(Duration::from_millis(100));
            running = process_is_running(pid);
        }
        running
    }

    #[cfg(windows)]
    fn grandchild_fixture_script() -> &'static str {
        "$p = Start-Process ping -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; \
         Write-Output $p.Id; Start-Sleep -Seconds 60"
    }

    #[cfg(windows)]
    fn grandchild_pid_from_stdout(stdout: &str) -> u32 {
        stdout
            .lines()
            .find_map(|line| line.trim().parse::<u32>().ok())
            .unwrap_or_else(|| panic!("expected a PID printed to stdout, got: {stdout:?}"))
    }

    #[test]
    #[cfg(windows)]
    fn timeout_terminates_grandchild_process_not_just_direct_child() {
        // PowerShell is the direct child; it starts `ping` as *its own*
        // child (a grandchild relative to Nova) and prints that grandchild's
        // PID before sleeping to hold the tree alive. Windows Job Objects
        // automatically include descendants unless they explicitly break
        // away (this script does not), so if timeout only killed the direct
        // child, `ping.exe` would keep running — this test proves it does
        // not.
        let root = temp_root("grandchild-timeout");
        let mut req = request(
            "powershell",
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                grandchild_fixture_script(),
            ],
            &root,
        );
        req.timeout = Duration::from_secs(5);
        let result = execute_blocking(req, CancellationToken::new());
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => assert_eq!(kind, ExecutionErrorKind::TimedOut),
            other => panic!("expected TimedOut, got {other:?}"),
        }
        let grandchild_pid = grandchild_pid_from_stdout(&result.stdout);
        let still_running = wait_until_not_running(grandchild_pid, 30);
        assert!(
            !still_running,
            "grandchild ping.exe (pid {grandchild_pid}) survived job termination on timeout"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(windows)]
    fn cancellation_terminates_grandchild_process_not_just_direct_child() {
        let root = temp_root("grandchild-cancel");
        let mut req = request(
            "powershell",
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                grandchild_fixture_script(),
            ],
            &root,
        );
        req.timeout = Duration::from_secs(60);
        let cancel = CancellationToken::new();
        let cancel_for_thread = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(2));
            cancel_for_thread.cancel();
        });
        let result = execute_blocking(req, cancel);
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::Cancelled)
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
        let grandchild_pid = grandchild_pid_from_stdout(&result.stdout);
        let still_running = wait_until_not_running(grandchild_pid, 30);
        assert!(
            !still_running,
            "grandchild ping.exe (pid {grandchild_pid}) survived job termination on cancel"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
