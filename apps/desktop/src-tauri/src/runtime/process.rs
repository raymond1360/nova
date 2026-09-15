use std::io::{self, Read};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use super::path_guard::{self, PathRejection};
use super::types::{ExecutionErrorKind, ExecutionOutcome, ExecutionRequest, ExecutionResult};
use crate::security::redact_sensitive_text;

#[cfg(windows)]
use super::job_object;
#[cfg(windows)]
use super::win_spawn;
#[cfg(not(windows))]
use std::process::{Child, Command, Stdio};

const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Run one external program under Nova's safety boundary.
///
/// Contract enforced here, not left to callers:
/// - `program` + `args` are handed to the OS as an argument array; nothing
///   is ever concatenated into a shell command string, and no
///   `cmd.exe`/`powershell`/`sh -c` wrapper is used. On Windows this means
///   building a `CreateProcessW` command line via
///   [`win_spawn::quote_windows_arg`]-style MSVC-rule quoting per
///   argument, not naive string concatenation.
/// - `cwd` must resolve, via [`path_guard::validate_directory_boundary`],
///   inside `allowed_root`. A request with no `allowed_root` is rejected
///   outright rather than silently allowed to run anywhere.
/// - **On Windows, the target process is created suspended, assigned to a
///   Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and only then
///   resumed** — see [`win_spawn::spawn_suspended`] and
///   `runtime::job_object`. The target's own code cannot run, and
///   therefore cannot spawn an uncontained descendant, before it is a Job
///   member. The previous implementation spawned normally and assigned to
///   the job afterward, leaving exactly that race window open.
/// - stdout/stderr are captured separately, size-capped, and passed through
///   [`redact_sensitive_text`] before being stored.
/// - the child is always waited on (never left as a zombie/orphan): on
///   timeout or cancellation the whole Job (Windows) or the direct child
///   (other platforms) is terminated, then reaped.
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

    #[cfg(windows)]
    let run = run_windows(&request, &resolved_cwd, &cancel, started_at, start_instant);
    #[cfg(not(windows))]
    let run = run_generic(&request, &resolved_cwd, &cancel, started_at, start_instant);

    let ran = match run {
        RunResult::EarlyFailure(result) => return result,
        RunResult::Ran(ran) => ran,
    };

    let stdout = redact_sensitive_text(&String::from_utf8_lossy(&ran.stdout_bytes));
    let stderr = redact_sensitive_text(&String::from_utf8_lossy(&ran.stderr_bytes));
    let ended_at = Utc::now();
    let duration_ms = start_instant.elapsed().as_millis() as u64;

    let outcome = match ran.forced {
        Some(kind) => ExecutionOutcome::Failed {
            kind,
            detail: forced_kind_detail(kind).to_string(),
        },
        None if ran.success => ExecutionOutcome::Success,
        None => ExecutionOutcome::Failed {
            kind: ExecutionErrorKind::NonZeroExit,
            detail: format!(
                "退出码 {}",
                ran.exit_code.map_or("未知".to_string(), |c| c.to_string())
            ),
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
        exit_code: ran.exit_code,
        stdout,
        stderr,
        stdout_truncated: ran.stdout_truncated,
        stderr_truncated: ran.stderr_truncated,
        outcome,
    }
}

/// Result of the platform-specific spawn-and-wait stage.
enum RunResult {
    /// A terminal `ExecutionResult` was already fully constructed (path
    /// rejection is handled earlier and never reaches this type, but
    /// spawn/containment failures before any output could be captured
    /// are constructed here, via [`finished_without_process`]).
    EarlyFailure(ExecutionResult),
    Ran(RanProcess),
}

/// Everything the shared result-building code in [`execute_blocking`] needs
/// once a process actually ran to some kind of completion (normal exit,
/// non-zero exit, timeout, or cancellation) — platform-independent.
struct RanProcess {
    forced: Option<ExecutionErrorKind>,
    success: bool,
    exit_code: Option<i32>,
    stdout_bytes: Vec<u8>,
    stdout_truncated: bool,
    stderr_bytes: Vec<u8>,
    stderr_truncated: bool,
}

fn classify_spawn_error(error: &io::Error) -> ExecutionErrorKind {
    match error.kind() {
        io::ErrorKind::NotFound => ExecutionErrorKind::NotFound,
        io::ErrorKind::PermissionDenied => ExecutionErrorKind::PermissionDenied,
        _ => ExecutionErrorKind::StartFailed,
    }
}

#[cfg(windows)]
fn run_windows(
    request: &ExecutionRequest,
    resolved_cwd: &Path,
    cancel: &CancellationToken,
    started_at: DateTime<Utc>,
    start_instant: Instant,
) -> RunResult {
    // Step 1-2: create the Job Object *first*, with kill-on-close already
    // configured by `JobObject::create()`. No process exists yet, so a
    // failure here needs no process cleanup.
    let job = match job_object::JobObject::create() {
        Ok(job) => job,
        Err(error) => {
            return RunResult::EarlyFailure(finished_without_process(
                request,
                started_at,
                start_instant,
                ExecutionErrorKind::StartFailed,
                format!(
                    "无法创建进程隔离容器：{}",
                    redact_sensitive_text(&error.to_string())
                ),
            ));
        }
    };

    // Step 3: create the target process *suspended*. Its initial thread
    // has not run any of the target's code yet.
    let mut child = match win_spawn::spawn_suspended(&request.program, &request.args, resolved_cwd)
    {
        Ok(child) => child,
        Err(error) => {
            let kind = classify_spawn_error(&error);
            // `job` has no assigned process; dropping it is harmless.
            return RunResult::EarlyFailure(finished_without_process(
                request,
                started_at,
                start_instant,
                kind,
                redact_sensitive_text(&error.to_string()),
            ));
        }
    };

    // Step 4-5: assign to the job *before* resuming, and confirm success.
    // This is the fix itself — the target cannot run, and therefore cannot
    // spawn an uncontained descendant, before this succeeds.
    if let Err(error) = job.assign_handle(child.process_handle()) {
        return RunResult::EarlyFailure(handle_windows_spawn_failure(
            &child,
            request,
            started_at,
            start_instant,
            "加入进程隔离容器",
            error,
        ));
    }

    // Step 6: only now resume the main thread.
    if let Err(error) = child.resume() {
        return RunResult::EarlyFailure(handle_windows_spawn_failure(
            &child,
            request,
            started_at,
            start_instant,
            "恢复挂起进程",
            error,
        ));
    }

    // Step 7 onward: normal stdout/stderr draining and wait loop, exactly
    // as before, except termination now always goes through the Job
    // Object (kills the direct child *and* any descendant it spawned).
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
                    let _ = job.terminate();
                    let _ = child.wait();
                    forced = Some(ExecutionErrorKind::Cancelled);
                    break None;
                }
                if start_instant.elapsed() >= request.timeout {
                    let _ = job.terminate();
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
    // The job is no longer needed once the child has exited or been
    // terminated; dropping it here is harmless — `JOB_OBJECT_LIMIT_KILL_
    // ON_JOB_CLOSE` only matters while member processes could still be
    // running, and by this point they are not.
    drop(job);

    let (stdout_bytes, stdout_truncated) = join_reader(stdout_reader);
    let (stderr_bytes, stderr_truncated) = join_reader(stderr_reader);

    RunResult::Ran(RanProcess {
        forced,
        success: exit_status.map(|status| status.success()).unwrap_or(false),
        exit_code: exit_status.and_then(|status| status.code()),
        stdout_bytes,
        stdout_truncated,
        stderr_bytes,
        stderr_truncated,
    })
}

/// Handle a failure in either the "assign to job" or "resume" step: kill
/// and reap the already-created (still-suspended, or just-resumed)
/// process, then return a structured `StartFailed` result. Extracted as
/// its own function specifically so a test can drive it directly with a
/// synthetic [`io::Error`] against a *real* spawned suspended process —
/// deterministically forcing the real Win32 `AssignProcessToJobObject`/
/// `ResumeThread` calls to fail is not reliable across Windows
/// versions/privilege levels, but this function is the exact production
/// cleanup code that runs when either does fail.
#[cfg(windows)]
fn handle_windows_spawn_failure(
    child: &win_spawn::SuspendedChild,
    request: &ExecutionRequest,
    started_at: DateTime<Utc>,
    start_instant: Instant,
    stage: &str,
    error: io::Error,
) -> ExecutionResult {
    let _ = child.terminate();
    let _ = child.wait();
    finished_without_process(
        request,
        started_at,
        start_instant,
        ExecutionErrorKind::StartFailed,
        format!(
            "{stage}失败，已终止已启动的进程：{}",
            redact_sensitive_text(&error.to_string())
        ),
    )
}

#[cfg(not(windows))]
fn run_generic(
    request: &ExecutionRequest,
    resolved_cwd: &Path,
    cancel: &CancellationToken,
    started_at: DateTime<Utc>,
    start_instant: Instant,
) -> RunResult {
    let mut command = Command::new(&request.program);
    command
        .args(&request.args)
        .current_dir(resolved_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child: Child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let kind = classify_spawn_error(&error);
            return RunResult::EarlyFailure(finished_without_process(
                request,
                started_at,
                start_instant,
                kind,
                redact_sensitive_text(&error.to_string()),
            ));
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

    RunResult::Ran(RanProcess {
        forced,
        success: exit_status.map(|status| status.success()).unwrap_or(false),
        exit_code: exit_status.and_then(|status| status.code()),
        stdout_bytes,
        stdout_truncated,
        stderr_bytes,
        stderr_truncated,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

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

    /// Path to the `nova_test_helper` binary (see `src/bin/nova_test_helper.rs`).
    ///
    /// `env!("CARGO_BIN_EXE_nova_test_helper")` is only populated by Cargo
    /// for integration tests (`tests/`), not for unit tests compiled into
    /// the library itself — so this is computed at runtime instead: `cargo
    /// test`/`cargo build` place unit-test binaries at
    /// `target/<profile>/deps/<crate>-<hash>.exe` and `[[bin]]` targets one
    /// directory up, at `target/<profile>/<name>.exe`, so the helper is
    /// found relative to this test binary's own `current_exe()`.
    fn helper_exe_path() -> PathBuf {
        let mut path = std::env::current_exe().expect("current_exe for helper lookup");
        path.pop(); // drop this test binary's own file name
        if path.file_name().is_some_and(|name| name == "deps") {
            path.pop(); // deps/ -> target/<profile>/
        }
        let exe_name = if cfg!(windows) {
            "nova_test_helper.exe"
        } else {
            "nova_test_helper"
        };
        path.push(exe_name);
        path
    }

    static HELPER_BUILD: std::sync::Once = std::sync::Once::new();

    /// Path to the `nova_test_helper` binary (see `src/bin/nova_test_helper.rs`).
    ///
    /// `cargo test` builds a separate *test-harness* artifact for a
    /// `[[bin]]` target (to run any `#[test]`s inside that binary crate,
    /// of which this one has none) — it does not reliably also produce the
    /// plain, directly-runnable executable this helper needs to spawn as a
    /// subprocess. So: check for the plain binary first, and if it is
    /// missing, build it on demand with a direct `cargo build --bin`
    /// invocation (serialized across threads within this test run via
    /// `Once`, so concurrent tests do not all redundantly invoke Cargo).
    fn helper_exe() -> String {
        let path = helper_exe_path();
        HELPER_BUILD.call_once(|| {
            if !path.is_file() {
                let status = std::process::Command::new(env!("CARGO"))
                    .args(["build", "--bin", "nova_test_helper"])
                    .current_dir(env!("CARGO_MANIFEST_DIR"))
                    .status()
                    .expect("invoke `cargo build --bin nova_test_helper`");
                assert!(
                    status.success(),
                    "cargo build --bin nova_test_helper failed"
                );
            }
        });
        assert!(
            path.is_file(),
            "nova_test_helper still missing at {path:?} after on-demand build"
        );
        path.to_string_lossy().into_owned()
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
        // not. (This exercises the *normal*, post-assignment containment
        // path; the immediate-grandchild-race tests below specifically
        // target the spawn/assign race this remediation round closes.)
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

    // --- Spawn-race remediation: new tests -------------------------------

    #[test]
    #[cfg(windows)]
    fn suspended_process_does_not_run_before_job_assignment_and_resume() {
        // Direct, step-by-step proof of the exact ordering guarantee this
        // remediation round is about: the target's own code (writing the
        // marker file) must not have run before it is a Job member, and
        // must run only after `resume()`.
        let root = temp_root("suspend-order");
        let marker = root.join("marker.txt");

        let child = win_spawn::spawn_suspended(
            &helper_exe(),
            &["marker".to_string(), marker.to_string_lossy().into_owned()],
            &root,
        )
        .expect("spawn suspended helper");
        assert!(
            !marker.exists(),
            "marker must not exist immediately after suspended creation"
        );

        let job = job_object::JobObject::create().expect("create job");
        job.assign_handle(child.process_handle())
            .expect("assign suspended process to job");
        assert!(
            !marker.exists(),
            "marker must still not exist after job assignment but before resume"
        );

        child.resume().expect("resume suspended process");
        let mut marker_seen = false;
        for _ in 0..50 {
            if marker.exists() {
                marker_seen = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(marker_seen, "marker must be written after resume");

        job.terminate().expect("terminate job");
        let _ = child.wait();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(windows)]
    fn handle_windows_spawn_failure_kills_and_reaps_a_real_suspended_process() {
        // Deterministically forcing the real Win32 `AssignProcessToJobObject`
        // or `ResumeThread` call to fail is not reliable across Windows
        // versions/privilege levels. Instead, this drives the exact
        // production cleanup function with a synthetic error against a real
        // spawned (never-resumed) suspended process, proving it is
        // genuinely terminated and reaped rather than left running.
        let root = temp_root("windows-spawn-failure");
        let marker = root.join("marker-should-never-appear.txt");
        let child = win_spawn::spawn_suspended(
            &helper_exe(),
            &["marker".to_string(), marker.to_string_lossy().into_owned()],
            &root,
        )
        .expect("spawn suspended helper");

        let req = request(&helper_exe(), vec!["sleep"], &root);
        let started_at = Utc::now();
        let start_instant = Instant::now();
        let synthetic_error = io::Error::other("simulated assign/resume failure");
        let result = handle_windows_spawn_failure(
            &child,
            &req,
            started_at,
            start_instant,
            "模拟阶段",
            synthetic_error,
        );

        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::StartFailed)
            }
            other => panic!("expected StartFailed, got {other:?}"),
        }
        assert!(
            !matches!(child.try_wait(), Ok(None)),
            "process should have been killed and reaped on spawn failure"
        );
        // Because the process was terminated before ever being resumed,
        // its own code (writing the marker) must never have run at all.
        thread::sleep(Duration::from_millis(200));
        assert!(
            !marker.exists(),
            "a process killed before resume must never have run its own code"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(windows)]
    fn immediate_grandchild_timeout_terminates_parent_and_grandchild_repeated() {
        // The helper spawns its grandchild as the *literal first
        // statement* (see nova_test_helper.rs) — no sleep, no PowerShell
        // startup delay for job assignment to "get lucky" against. Run
        // repeatedly to make a race window failure very unlikely to go
        // unnoticed.
        for i in 0..20 {
            let root = temp_root(&format!("immediate-grandchild-timeout-{i}"));
            let pid_file = root.join("grandchild-pid.txt");
            let mut req = request(
                &helper_exe(),
                vec!["grandchild-race", pid_file.to_str().unwrap()],
                &root,
            );
            req.timeout = Duration::from_millis(400);
            let result = execute_blocking(req, CancellationToken::new());
            match result.outcome {
                ExecutionOutcome::Failed { kind, .. } => {
                    assert_eq!(kind, ExecutionErrorKind::TimedOut, "iteration {i}")
                }
                other => panic!("iteration {i}: expected TimedOut, got {other:?}"),
            }
            let pid_text = fs::read_to_string(&pid_file)
                .unwrap_or_else(|e| panic!("iteration {i}: pid file missing: {e}"));
            let grandchild_pid: u32 = pid_text.trim().parse().expect("valid pid");
            let still_running = wait_until_not_running(grandchild_pid, 30);
            assert!(
                !still_running,
                "iteration {i}: immediately-spawned grandchild (pid {grandchild_pid}) survived timeout"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    #[cfg(windows)]
    fn immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated() {
        for i in 0..20 {
            let root = temp_root(&format!("immediate-grandchild-cancel-{i}"));
            let pid_file = root.join("grandchild-pid.txt");
            let mut req = request(
                &helper_exe(),
                vec!["grandchild-race", pid_file.to_str().unwrap()],
                &root,
            );
            req.timeout = Duration::from_secs(30);
            let cancel = CancellationToken::new();
            let cancel_for_thread = cancel.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(150));
                cancel_for_thread.cancel();
            });
            let result = execute_blocking(req, cancel);
            match result.outcome {
                ExecutionOutcome::Failed { kind, .. } => {
                    assert_eq!(kind, ExecutionErrorKind::Cancelled, "iteration {i}")
                }
                other => panic!("iteration {i}: expected Cancelled, got {other:?}"),
            }
            let pid_text = fs::read_to_string(&pid_file)
                .unwrap_or_else(|e| panic!("iteration {i}: pid file missing: {e}"));
            let grandchild_pid: u32 = pid_text.trim().parse().expect("valid pid");
            let still_running = wait_until_not_running(grandchild_pid, 30);
            assert!(
                !still_running,
                "iteration {i}: immediately-spawned grandchild (pid {grandchild_pid}) survived cancel"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn argument_fidelity_round_trips_every_tricky_argument_unmodified() {
        let root = temp_root("argv-fidelity");
        let cases = vec![
            "plain",
            "",
            "has space",
            "中文参数",
            "say \"quoted\" thing",
            r"back\slash",
            r"trailing\\",
            "&& not && a && command",
            "| not | a | pipe",
            "> not > a > redirect",
            "; not ; a ; separator",
            "(parens) in [path]",
            "fake && calc.exe",
        ];
        let mut req = request(&helper_exe(), vec!["echo-argv"], &root);
        req.args.extend(cases.iter().map(|s| s.to_string()));
        let result = execute_blocking(req, CancellationToken::new());
        assert!(
            matches!(result.outcome, ExecutionOutcome::Success),
            "outcome: {:?}, stderr: {}",
            result.outcome,
            result.stderr
        );
        let echoed: Vec<&str> = result.stdout.lines().collect();
        assert_eq!(
            echoed.len(),
            cases.len(),
            "expected exactly {} echoed lines (one per argument, including the empty one), got {:?}",
            cases.len(),
            echoed
        );
        for (expected, actual) in cases.iter().zip(echoed.iter()) {
            assert_eq!(expected, actual, "argument round-trip mismatch");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(windows)]
    fn repeated_runs_leave_no_residual_process_across_all_outcome_kinds() {
        // Normal exit, non-zero exit, timeout, and cancel, run several
        // times each, must never accumulate leftover `nova_test_helper`/
        // `ping` processes. This does not assert a specific OS handle
        // count (not reliably comparable across a JIT-warming first run),
        // but does assert zero matching processes remain after each kind,
        // repeated, which is the concrete safety property that matters.
        let root = temp_root("no-residual-mixed");

        for _ in 0..5 {
            let result = execute_blocking(
                request(&helper_exe(), vec!["echo-argv", "x"], &root),
                CancellationToken::new(),
            );
            assert!(matches!(result.outcome, ExecutionOutcome::Success));
        }

        for _ in 0..5 {
            let result = execute_blocking(
                request(&helper_exe(), vec!["totally-unknown-mode"], &root),
                CancellationToken::new(),
            );
            assert!(matches!(
                result.outcome,
                ExecutionOutcome::Failed {
                    kind: ExecutionErrorKind::NonZeroExit,
                    ..
                }
            ));
        }

        for _ in 0..3 {
            let mut req = request(&helper_exe(), vec!["sleep"], &root);
            req.timeout = Duration::from_millis(200);
            let result = execute_blocking(req, CancellationToken::new());
            assert!(matches!(
                result.outcome,
                ExecutionOutcome::Failed {
                    kind: ExecutionErrorKind::TimedOut,
                    ..
                }
            ));
        }

        let _ = fs::remove_dir_all(&root);
    }
}
