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
    let mut child = match win_spawn::spawn_suspended(
        &request.program,
        &request.args,
        resolved_cwd,
        request.stdin.is_some(),
    ) {
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

    // If the caller provided stdin content, write it on its own thread
    // (never inline, never blocking this function) and then drop the pipe
    // to close it — signaling EOF to the child. A dedicated thread avoids a
    // deadlock: if the child also writes enough stdout/stderr to fill its
    // own pipe buffers before it has finished reading stdin, a synchronous
    // write here could block forever waiting for buffer space that only
    // frees up once *this* function starts draining stdout/stderr.
    let stdin_writer = child.stdin.take().map(|mut pipe| {
        let content = request.stdin.clone().unwrap_or_default();
        thread::Builder::new()
            .name("nova-process-stdin-writer".to_string())
            .spawn(move || {
                use std::io::Write as _;
                let _ = pipe.write_all(&content);
                // Dropping `pipe` here closes the parent's write handle,
                // which is the child's only signal that stdin has ended.
            })
    });

    // Step 7 onward: normal stdout/stderr draining and wait loop, exactly
    // as before, except termination now always goes through the Job
    // Object (kills the direct child *and* any descendant it spawned).
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let (stdout_reader, stderr_reader) =
        match spawn_reader_pair(stdout_pipe, stderr_pipe, request.max_output_bytes) {
            Ok(readers) => readers,
            Err(error) => {
                // The process has already been resumed and may have
                // spawned its own descendants by now, so cleanup must go
                // through the Job Object (kills the whole tree), not just
                // the direct child.
                let _ = job.terminate();
                let _ = child.wait();
                return RunResult::EarlyFailure(finished_without_process(
                    request,
                    started_at,
                    start_instant,
                    ExecutionErrorKind::StartFailed,
                    format!(
                        "无法创建输出读取线程，已终止进程：{}",
                        redact_sensitive_text(&error.to_string())
                    ),
                ));
            }
        };

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

    // Best-effort join: by the time the child has exited, its stdin has
    // necessarily already been fully read or the child never needed the
    // rest of it — either way there is nothing further for this thread to
    // do, and joining just reclaims it rather than leaking a handle.
    if let Some(Ok(handle)) = stdin_writer {
        let _ = handle.join();
    }

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
        .stdin(if request.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
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

    let stdin_writer = child.stdin.take().map(|mut pipe| {
        let content = request.stdin.clone().unwrap_or_default();
        thread::Builder::new()
            .name("nova-process-stdin-writer".to_string())
            .spawn(move || {
                use std::io::Write as _;
                let _ = pipe.write_all(&content);
            })
    });

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let (stdout_reader, stderr_reader) =
        match spawn_reader_pair(stdout_pipe, stderr_pipe, request.max_output_bytes) {
            Ok(readers) => readers,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return RunResult::EarlyFailure(finished_without_process(
                    request,
                    started_at,
                    start_instant,
                    ExecutionErrorKind::StartFailed,
                    format!(
                        "无法创建输出读取线程，已终止进程：{}",
                        redact_sensitive_text(&error.to_string())
                    ),
                ));
            }
        };

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

    if let Some(Ok(handle)) = stdin_writer {
        let _ = handle.join();
    }

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

/// Spawns a capped output-draining thread via `Builder::spawn`, which
/// returns `io::Result` instead of panicking when the OS cannot create a
/// new thread (unlike bare `thread::spawn`). Callers must handle the
/// `Err` case by tearing down the already-started process/job — never by
/// unwrapping or panicking.
fn spawn_capped_reader<R>(mut reader: R, cap: usize) -> io::Result<ReaderHandle>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name("nova-process-reader".to_string())
        .spawn(move || {
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

/// Spawns the stdout/stderr reader threads together. If the stderr reader
/// fails to spawn after the stdout reader already started, the started
/// stdout reader is joined before returning the error so no thread is
/// ever left behind.
fn spawn_reader_pair<A, B>(
    stdout_pipe: Option<A>,
    stderr_pipe: Option<B>,
    cap: usize,
) -> io::Result<(Option<ReaderHandle>, Option<ReaderHandle>)>
where
    A: Read + Send + 'static,
    B: Read + Send + 'static,
{
    spawn_reader_pair_with(
        stdout_pipe,
        stderr_pipe,
        cap,
        spawn_capped_reader,
        spawn_capped_reader,
    )
}

/// Same as [`spawn_reader_pair`], but with the two thread-spawning
/// operations passed in explicitly. Production code always goes through
/// [`spawn_reader_pair`], which wires up the real
/// [`spawn_capped_reader`] for both sides — this indirection exists
/// solely so tests can inject a synthetic thread-creation failure (real
/// OS-level thread-creation failure is not reliably reproducible in a
/// test, the same reasoning already applied to
/// `handle_windows_spawn_failure` above) without adding any
/// failure-injection surface to the production API.
fn spawn_reader_pair_with<A, B, FA, FB>(
    stdout_pipe: Option<A>,
    stderr_pipe: Option<B>,
    cap: usize,
    spawn_stdout: FA,
    spawn_stderr: FB,
) -> io::Result<(Option<ReaderHandle>, Option<ReaderHandle>)>
where
    A: Read + Send + 'static,
    B: Read + Send + 'static,
    FA: FnOnce(A, usize) -> io::Result<ReaderHandle>,
    FB: FnOnce(B, usize) -> io::Result<ReaderHandle>,
{
    let stdout_reader = match stdout_pipe {
        Some(pipe) => Some(spawn_stdout(pipe, cap)?),
        None => None,
    };
    let stderr_reader = match stderr_pipe {
        Some(pipe) => match spawn_stderr(pipe, cap) {
            Ok(handle) => Some(handle),
            Err(error) => {
                if let Some(handle) = stdout_reader {
                    let _ = handle.join();
                }
                return Err(error);
            }
        },
        None => None,
    };
    Ok((stdout_reader, stderr_reader))
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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
                // `nova_test_helper` is gated behind the non-default
                // `nova-test-helper` feature specifically so it is never
                // built by an ordinary `cargo build --release`/
                // `npm run tauri -- build` — those must never enable this
                // feature. `cargo test` does not enable it either (it is
                // not in `default = []`), so it is built on demand here,
                // explicitly, with the feature turned on for this one
                // invocation only.
                let status = std::process::Command::new(env!("CARGO"))
                    .args([
                        "build",
                        "--bin",
                        "nova_test_helper",
                        "--features",
                        "nova-test-helper",
                    ])
                    .current_dir(env!("CARGO_MANIFEST_DIR"))
                    .status()
                    .expect(
                        "invoke `cargo build --bin nova_test_helper --features nova-test-helper`",
                    );
                assert!(
                    status.success(),
                    "cargo build --bin nova_test_helper --features nova-test-helper failed"
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

    #[cfg(windows)]
    #[test]
    fn npm_resolves_past_its_own_extensionless_shell_script_stand_in() {
        // Real bug found via re-review testing: Node.js's Windows installer
        // ships a bare, extension-less `npm` file (a POSIX shell script for
        // WSL/git-bash use) right next to the real `npm.cmd`. The original
        // `resolve_executable` tried the bare name in each PATH directory
        // *before* trying `PATHEXT` extensions, so it could resolve `npm`
        // to that non-Win32-executable shell script and fail every launch
        // with `ERROR_BAD_EXE_FORMAT` ("%1 is not a valid Win32
        // application") — not a test failure, the process never started.
        let root = temp_root("npm-resolves-real-cmd");
        let req = request("npm", vec!["--version"], &root);
        let result = execute_blocking(req, CancellationToken::new());
        assert!(
            matches!(result.outcome, ExecutionOutcome::Success),
            "outcome: {:?}, stderr: {}",
            result.outcome,
            result.stderr
        );
        assert_eq!(result.exit_code, Some(0));
        assert!(
            !result.stdout.trim().is_empty(),
            "expected npm to print a version string"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn npm_test_actually_runs_inside_the_requested_cwd_not_windows_system32() {
        // Real bug found via re-review testing, discovered only after
        // fixing the two bugs above: `path_guard::validate_directory_
        // boundary`'s canonicalized `cwd` carries Windows's `\\?\`
        // extended-length prefix. `cmd.exe` (the interpreter every batch
        // script launch above routes through) refuses that prefix as its
        // *own* working directory — it prints "UNC paths are not
        // supported... defaulting to Windows directory" and silently runs
        // every child (here, `npm test` -> `node`) from `%SystemRoot%`
        // instead. A project's real test command would then fail with a
        // spurious "no package.json found" even though the project
        // directory Nova passed in was completely correct.
        let root = temp_root("npm-test-real-cwd");
        fs::write(
            root.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"console.log(process.cwd())\""}}"#,
        )
        .unwrap();
        let req = request("npm", vec!["test"], &root);
        let result = execute_blocking(req, CancellationToken::new());
        assert!(
            matches!(result.outcome, ExecutionOutcome::Success),
            "outcome: {:?}, stdout: {}, stderr: {}",
            result.outcome,
            result.stdout,
            result.stderr
        );
        // `win_spawn::win32_cwd_form` deliberately strips the `\\?\`
        // extended-length prefix `fs::canonicalize` adds before handing a
        // directory to `CreateProcessW` (see that function's doc comment),
        // so the real child process's own `process.cwd()` never has it
        // either — the expectation here must go through the same
        // transform rather than the raw canonicalized path.
        let canonical_root = fs::canonicalize(&root).unwrap();
        let expected_cwd = win_spawn::win32_cwd_form(&canonical_root);
        let printed_cwd = result
            .stdout
            .lines()
            .find(|line| line.contains(':') && line.contains('\\'))
            .unwrap_or_default()
            .trim();
        assert_eq!(
            printed_cwd,
            expected_cwd,
            "the node process must actually run inside the project directory, not %SystemRoot%; full stdout: {:?}",
            result.stdout
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn a_real_windows_batch_script_actually_runs_not_just_resolves() {
        // Real bug found while re-testing Blocker 3 against an npm-based
        // project: `npm`/`pnpm`/`yarn` all resolve to a `.cmd` file on
        // Windows, and `CreateProcessW` cannot launch a `.cmd` directly via
        // `lpApplicationName` (`ERROR_BAD_EXE_FORMAT`) — every auto-detected
        // npm/pnpm/yarn test command would fail to even start, not because
        // the tests failed, but because the process never ran. This proves
        // the `cmd.exe /d /s /c` routing in `win_spawn::spawn_suspended`
        // actually executes a real `.cmd` file end to end, with a real
        // argument reaching the script.
        let root = temp_root("batch-script-real-run");
        let script_path = root.join("nova_test_probe.cmd");
        fs::write(&script_path, "@echo off\r\necho got:%1\r\n").unwrap();
        let req = request(
            script_path.to_str().unwrap(),
            vec!["hello-from-nova"],
            &root,
        );
        let result = execute_blocking(req, CancellationToken::new());
        assert!(
            matches!(result.outcome, ExecutionOutcome::Success),
            "outcome: {:?}, stdout: {}, stderr: {}",
            result.outcome,
            result.stdout,
            result.stderr
        );
        assert!(
            result.stdout.contains("got:hello-from-nova"),
            "expected the batch script's own echo of its argument, got stdout: {:?}",
            result.stdout
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn a_batch_script_argument_containing_a_double_quote_is_refused_before_spawn() {
        // Closes the residual injection surface a naive `cmd.exe /C`
        // wrapper would otherwise open: cmd.exe's own `&`/`|`/quote-toggle
        // parsing of the string after `/C` is a different, unrelated
        // grammar from `CreateProcessW`'s argv quoting (this exact gap is
        // what Node.js's CVE-2024-27980 was about). Rather than trying to
        // reconcile the two, an embedded `"` is refused outright.
        let root = temp_root("batch-script-quote-injection");
        let script_path = root.join("nova_test_probe.cmd");
        fs::write(&script_path, "@echo off\r\necho ran\r\n").unwrap();
        let req = request(script_path.to_str().unwrap(), vec!["a\"b"], &root);
        let result = execute_blocking(req, CancellationToken::new());
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::StartFailed)
            }
            other => panic!("expected a refused start, got {other:?}"),
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

    #[test]
    #[cfg(windows)]
    fn embedded_nul_in_an_argument_is_rejected_end_to_end_without_starting_a_process() {
        // Uses the real helper binary in "marker" mode as `program` (a
        // legitimate, existing executable) but with a NUL-containing
        // second argument. If the request were merely truncated rather
        // than rejected outright, the helper would still start (using
        // whatever survived up to the NUL) and write its marker file —
        // this test's whole point is proving that never happens.
        let root = temp_root("nul-end-to-end");
        let marker = root.join("marker-must-not-appear.txt");
        let mut req = request(&helper_exe(), vec!["marker", "placeholder"], &root);
        req.args[1] = format!("{}\0trailing", marker.to_string_lossy());
        let result = execute_blocking(req, CancellationToken::new());

        assert!(
            !matches!(result.outcome, ExecutionOutcome::Success),
            "a NUL-containing argument must never produce Success, got {:?}",
            result.outcome
        );
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::StartFailed)
            }
            other => panic!("expected StartFailed, got {other:?}"),
        }
        // Give a slow spawn plenty of time to prove a negative: no marker
        // file must appear, meaning the helper's own code never ran at all.
        thread::sleep(Duration::from_millis(500));
        assert!(
            !marker.exists(),
            "process must never have started (and therefore never written its marker) \
             for a request Nova rejected before CreateProcessW"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(windows)]
    fn embedded_nul_in_program_and_cwd_are_also_rejected_end_to_end() {
        let root = temp_root("nul-program-cwd");

        let req_program = request("git\0evil", vec!["--version"], &root);
        let result = execute_blocking(req_program, CancellationToken::new());
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::StartFailed)
            }
            other => panic!("expected StartFailed for NUL in program, got {other:?}"),
        }

        let mut req_cwd = request("git", vec!["--version"], &root);
        let mut bad_cwd = root.clone();
        bad_cwd.push("bad\0dir");
        req_cwd.cwd = bad_cwd;
        req_cwd.allowed_root = Some(root.clone());
        let result = execute_blocking(req_cwd, CancellationToken::new());
        assert!(!matches!(result.outcome, ExecutionOutcome::Success));

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

    /// Work Package 3 final review remediation (Major 2): replaces the
    /// previous "print the PID to stdout, then sleep" fixture, whose
    /// readiness signal was only ever observable *after* the whole process
    /// tree had already been torn down (`result.stdout` is only populated
    /// once `execute_blocking` returns). `cancellation_terminates_
    /// grandchild_process_not_just_direct_child` paired that with a fixed
    /// 2-second sleep before firing cancellation, gambling that PowerShell
    /// — a much heavier process to start than the `node`/helper binaries
    /// used elsewhere in this codebase's own marker-based tests — would
    /// have started, spawned `ping`, and flushed its `Write-Output` within
    /// that window on every possible run. Under full-parallel-test-suite
    /// CPU/process-creation contention this is not guaranteed: if
    /// cancellation fires before PowerShell's own `Start-Process`/
    /// `Write-Output` line ever executes, the process tree is killed with
    /// nothing ever written to stdout, and `grandchild_pid_from_stdout`
    /// panics on an empty string — a real, reproduced instance of exactly
    /// this (`CODEX_PHASE3_SINGLE_AI_FINAL_ACCEPTANCE_REVIEW09242120.md`
    /// Major 2, round 1 of a 5-round full-suite run: `expected a PID
    /// printed to stdout, got: ""`). This is a test-synchronization defect,
    /// not a production one — real cancellation/Job Object behavior was
    /// never in question (the *same* failing run's grandchild, once its
    /// PID could be determined, always turned out to have actually been
    /// terminated; `immediate_grandchild_cancel_terminates_parent_and_
    /// grandchild_repeated`, which already uses a real PID-file handshake
    /// for the same underlying race, has run stably for many rounds).
    ///
    /// The fix: the script itself now writes the grandchild's real PID to
    /// `pid_file` — atomically (a temp file, then an in-place rename, so a
    /// concurrent reader can never observe a partial write) — as its very
    /// next statement after starting `ping`, before ever sleeping. This is
    /// the exact same ready-signal discipline `nova_test_helper.rs`'s own
    /// `grandchild-race` mode and [`wait_for_readable_pid_file`] already
    /// use for the immediate-grandchild race tests below — reused here
    /// rather than inventing a second, divergent mechanism.
    #[cfg(windows)]
    fn grandchild_fixture_script(pid_file: &Path) -> String {
        let pid_file = pid_file.to_string_lossy().replace('\'', "''");
        format!(
            "$p = Start-Process ping -ArgumentList '-n','60','127.0.0.1' -PassThru -WindowStyle Hidden; \
             $tmp = '{pid_file}.tmp'; \
             Set-Content -Path $tmp -Value $p.Id -NoNewline -Encoding ascii; \
             Move-Item -Path $tmp -Destination '{pid_file}' -Force; \
             Start-Sleep -Seconds 60"
        )
    }

    #[test]
    #[cfg(windows)]
    fn timeout_terminates_grandchild_process_not_just_direct_child() {
        // PowerShell is the direct child; it starts `ping` as *its own*
        // child (a grandchild relative to Nova), atomically publishes that
        // grandchild's real PID via a file (never stdout — see
        // `grandchild_fixture_script`'s doc comment), then sleeps to hold
        // the tree alive. Windows Job Objects automatically include
        // descendants unless they explicitly break away (this script does
        // not), so if timeout only killed the direct child, `ping.exe`
        // would keep running — this test proves it does not. (This
        // exercises the *normal*, post-assignment containment path; the
        // immediate-grandchild-race tests below specifically target the
        // spawn/assign race a prior remediation round closed.)
        let root = temp_root("grandchild-timeout");
        let pid_file = root.join("grandchild-pid.txt");
        let mut req = request(
            "powershell",
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &grandchild_fixture_script(&pid_file),
            ],
            &root,
        );
        // Work Package 3 final review remediation (Major 2): 5 seconds was
        // observed to be insufficient for PowerShell to reliably start,
        // spawn `ping`, and complete the atomic PID-file write under heavy
        // parallel process-creation contention (reproduced directly: a
        // panic showing the fixture directory existing but completely
        // empty even after the full wait budget). 10 seconds gives real
        // headroom while still keeping this test's wall time bounded and
        // small relative to the script's own 60-second sleep.
        req.timeout = Duration::from_secs(10);
        let result = execute_blocking(req, CancellationToken::new());
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => assert_eq!(kind, ExecutionErrorKind::TimedOut),
            other => panic!("expected TimedOut, got {other:?}"),
        }
        // The 10-second timeout already gave PowerShell ample time to reach
        // and complete the atomic PID-file write well before Nova's own
        // timeout/kill path ran — this is a short, bounded confirmation
        // read, not a fresh open-ended wait.
        let grandchild_pid = wait_for_readable_pid_file(&pid_file, &root, Duration::from_secs(5));
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
        let pid_file = root.join("grandchild-pid.txt");
        let mut req = request(
            "powershell",
            vec![
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &grandchild_fixture_script(&pid_file),
            ],
            &root,
        );
        req.timeout = Duration::from_secs(60);
        let cancel = CancellationToken::new();
        let cancel_for_thread = cancel.clone();
        let pid_file_for_thread = pid_file.clone();
        let root_for_thread = root.clone();
        // The canceller thread itself discovers the grandchild's real PID
        // by waiting on the ready signal (bounded, generous timeout — not
        // unbounded, not a fixed blind sleep) before firing cancellation,
        // and hands that same PID back via the join handle, exactly
        // mirroring `immediate_grandchild_cancel_terminates_parent_and_
        // grandchild_repeated`'s established pattern. This guarantees
        // cancellation is only ever attributed to "the grandchild was
        // genuinely already running" — never ready-before-PowerShell-
        // started-it.
        let canceller = thread::spawn(move || {
            let pid = wait_for_readable_pid_file(
                &pid_file_for_thread,
                &root_for_thread,
                Duration::from_secs(30),
            );
            cancel_for_thread.cancel();
            pid
        });
        let result = execute_blocking(req, cancel);
        match result.outcome {
            ExecutionOutcome::Failed { kind, .. } => {
                assert_eq!(kind, ExecutionErrorKind::Cancelled)
            }
            other => panic!("expected Cancelled, got {other:?}"),
        }
        let grandchild_pid = canceller.join().expect("canceller thread panicked");
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
            false,
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
            false,
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
        //
        // Work Package 3 final review remediation (Major 2): the original
        // 400ms budget was reproduced failing directly (`iteration 0: pid
        // file missing: 系统找不到指定的文件`) under heavy parallel
        // process-creation contention — Nova's own suspend-then-Job-
        // Object-assign-then-resume sequence for spawning even this
        // lightweight helper (see `runtime::process::run_windows`'s own
        // doc comments) has real, load-sensitive kernel-call overhead, the
        // same class of cost the build-cancellation stability remediation
        // already diagnosed for heavier process chains. 2 seconds gives
        // real headroom while staying tiny relative to the helper's own
        // 120-second sleep, so the timeout path is still genuinely
        // exercised (never masked by natural completion).
        for i in 0..20 {
            let root = temp_root(&format!("immediate-grandchild-timeout-{i}"));
            let pid_file = root.join("grandchild-pid.txt");
            let mut req = request(
                &helper_exe(),
                vec!["grandchild-race", pid_file.to_str().unwrap()],
                &root,
            );
            req.timeout = Duration::from_secs(2);
            let result = execute_blocking(req, CancellationToken::new());
            match result.outcome {
                ExecutionOutcome::Failed { kind, .. } => {
                    assert_eq!(kind, ExecutionErrorKind::TimedOut, "iteration {i}")
                }
                other => panic!("iteration {i}: expected TimedOut, got {other:?}"),
            }
            // Short, bounded defensive re-read (not a rescue mechanism —
            // if the helper was genuinely killed before ever writing the
            // file, this still fails fast and clearly) purely to absorb
            // any residual filesystem-visibility lag right at the timeout
            // boundary, rather than a single unbuffered read.
            let grandchild_pid =
                wait_for_readable_pid_file(&pid_file, &root, Duration::from_secs(2));
            let still_running = wait_until_not_running(grandchild_pid, 30);
            assert!(
                !still_running,
                "iteration {i}: immediately-spawned grandchild (pid {grandchild_pid}) survived timeout"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    /// Codex re-review remediation (Major 4): bounded, polling wait for the
    /// grandchild's PID file to appear *with real, parseable content* —
    /// used instead of a fixed-duration sleep so cancellation is always
    /// fired strictly after the grandchild has demonstrably started and
    /// recorded its own PID, never before. Root cause of the flake this
    /// closes: the previous version fired cancellation from a timer thread
    /// after a fixed 150ms sleep, racing against the real wall-clock time
    /// needed for the OS to schedule the test process, spawn the helper,
    /// have the helper spawn its grandchild, and have the grandchild write
    /// its PID to disk — all of which can legitimately take longer than
    /// 150ms under full-suite parallel CPU contention (many other tests
    /// concurrently spawning real processes). When cancellation fired
    /// before the grandchild ever reached its own `fs::write` call, the
    /// entire process tree (including the not-yet-pid-file-writing
    /// grandchild) was correctly terminated by the Job Object — but the
    /// test then tried to read a PID file that could never have been
    /// written, producing `pid file missing: 系统找不到指定的文件` on an
    /// otherwise fully correct cancellation. Nothing in the production Job
    /// Object containment logic needed to change: Windows automatically
    /// (and atomically, at `CreateProcess` time) adds any child a job
    /// member spawns to the same job — `runtime::win_spawn`/`job_object`
    /// set no `JOB_OBJECT_LIMIT_BREAKAWAY_OK`/silent-breakaway flag
    /// anywhere, so a grandchild that was ever actually created is always
    /// reliably captured and killed regardless of timing. This is a test
    /// synchronization fix only.
    /// Work Package 3 final review remediation (Major 2): `dir` is the
    /// fixture's own temp directory (not necessarily `path`'s immediate
    /// parent in every caller) — on timeout, the panic message reports the
    /// PID file's full path, how long was actually waited, whether the
    /// fixture directory itself still exists, and a listing of whatever is
    /// in it (a genuine race leaves the directory present but the PID file
    /// missing/empty; a fixture setup bug tends to leave the directory
    /// itself missing) — enough to distinguish "the write genuinely hadn't
    /// happened yet" from "the wrong path was ever checked" without
    /// printing any process argv/env content that could carry secrets.
    fn wait_for_readable_pid_file(path: &Path, dir: &Path, timeout: Duration) -> u32 {
        let started = Instant::now();
        let deadline = started + timeout;
        loop {
            if let Ok(text) = fs::read_to_string(path) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    if let Ok(pid) = trimmed.parse::<u32>() {
                        return pid;
                    }
                }
            }
            if Instant::now() >= deadline {
                let dir_listing = fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_else(|e| format!("<failed to list dir: {e}>"));
                panic!(
                    "grandchild pid file never became ready within {timeout:?} (waited {:?})\n  \
                     pid file path: {}\n  \
                     fixture dir: {} (exists: {}, entries: [{dir_listing}])",
                    started.elapsed(),
                    path.display(),
                    dir.display(),
                    dir.exists(),
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    #[cfg(windows)]
    fn wait_for_readable_pid_file_times_out_with_rich_diagnostics_when_the_file_never_appears() {
        // Work Package 3 final review remediation (Major 2, required test
        // item 6): deterministically exercises the timeout/panic path
        // itself (a short 150ms budget, no real process involved) and
        // asserts the panic message actually carries the required
        // diagnostics — never a bare "timed out".
        let root = temp_root("pid-file-timeout-diagnostics");
        fs::write(root.join("sentinel.txt"), "present").unwrap();
        let pid_file = root.join("grandchild-pid.txt");

        let path_for_closure = pid_file.clone();
        let dir_for_closure = root.clone();
        let result = std::panic::catch_unwind(move || {
            wait_for_readable_pid_file(
                &path_for_closure,
                &dir_for_closure,
                Duration::from_millis(150),
            )
        });
        let panic_payload = result.expect_err("must panic when the pid file never appears");
        let message = panic_payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic_payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();

        assert!(
            message.contains("grandchild pid file never became ready"),
            "message: {message}"
        );
        assert!(
            message.contains(&pid_file.to_string_lossy().into_owned()),
            "message: {message}"
        );
        assert!(
            message.contains(&root.to_string_lossy().into_owned()),
            "message: {message}"
        );
        assert!(message.contains("sentinel.txt"), "message: {message}");
        assert!(message.contains("waited"), "message: {message}");
        let _ = fs::remove_dir_all(&root);
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
            let pid_file_for_thread = pid_file.clone();
            let root_for_thread = root.clone();
            // The canceller thread itself discovers the grandchild's real
            // PID (waiting as long as it genuinely takes, up to a generous
            // bounded timeout) before firing cancellation, and hands that
            // same PID back via the join handle — the main thread never
            // needs to re-read the PID file after the fact, so there is no
            // window in which "the grandchild hadn't written it yet" can
            // be misread as "the file is missing".
            let canceller = thread::spawn(move || {
                let pid = wait_for_readable_pid_file(
                    &pid_file_for_thread,
                    &root_for_thread,
                    Duration::from_secs(10),
                );
                cancel_for_thread.cancel();
                pid
            });
            let result = execute_blocking(req, cancel);
            match result.outcome {
                ExecutionOutcome::Failed { kind, .. } => {
                    assert_eq!(kind, ExecutionErrorKind::Cancelled, "iteration {i}")
                }
                other => panic!("iteration {i}: expected Cancelled, got {other:?}"),
            }
            let grandchild_pid = canceller.join().expect("canceller thread panicked");
            let still_running = wait_until_not_running(grandchild_pid, 30);
            assert!(
                !still_running,
                "iteration {i}: immediately-spawned grandchild (pid {grandchild_pid}) survived cancel"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn stdin_content_is_written_to_the_child_and_read_back_via_stdout() {
        // Proves the Phase 3 remediation stdin-piping addition end to end:
        // content set via `ExecutionRequest::with_stdin` really does arrive
        // at the child process's stdin (not a file path, not an argv
        // element) — the exact property Blocker 2 requires of the real
        // Codex/Claude CLI adapters.
        let root = temp_root("stdin-roundtrip");
        let content = "task package content\n中文 with spaces\nline three"
            .as_bytes()
            .to_vec();
        let req = request(&helper_exe(), vec!["cat-stdin"], &root).with_stdin(content.clone());
        let result = execute_blocking(req, CancellationToken::new());
        assert!(
            matches!(result.outcome, ExecutionOutcome::Success),
            "outcome: {:?}, stderr: {}",
            result.outcome,
            result.stderr
        );
        assert_eq!(result.stdout.as_bytes(), content.as_slice());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_stdin_content_still_behaves_exactly_as_before_this_field_existed() {
        let root = temp_root("stdin-none");
        let req = request(&helper_exe(), vec!["cat-stdin"], &root);
        let result = execute_blocking(req, CancellationToken::new());
        assert!(matches!(result.outcome, ExecutionOutcome::Success));
        assert_eq!(
            result.stdout, "",
            "no stdin content means nothing to echo back"
        );
        let _ = fs::remove_dir_all(&root);
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

    // --- Reader-thread creation failure (Minor: `spawn_capped_reader` must
    // never panic on OS thread-creation failure) --------------------------
    //
    // A real OS-level thread-creation failure (resource exhaustion) is not
    // reliably reproducible in a test — the same limitation already applies
    // to `AssignProcessToJobObject`/`ResumeThread` above, whose cleanup
    // (`handle_windows_spawn_failure`) is likewise exercised with a
    // synthetic `io::Error` rather than a forced real failure. The tests
    // below apply that identical approach to `spawn_reader_pair_with`,
    // which is the exact production cleanup/propagation code that runs
    // when either reader thread fails to spawn.

    #[test]
    fn spawn_reader_pair_with_propagates_stdout_reader_creation_failure_without_touching_stderr() {
        let stderr_attempted = Arc::new(AtomicBool::new(false));
        let stderr_attempted_clone = Arc::clone(&stderr_attempted);

        let result = spawn_reader_pair_with(
            Some(io::Cursor::new(Vec::<u8>::new())),
            Some(io::Cursor::new(Vec::<u8>::new())),
            1024,
            |_pipe: io::Cursor<Vec<u8>>, _cap: usize| -> io::Result<ReaderHandle> {
                Err(io::Error::other(
                    "synthetic stdout reader thread-creation failure",
                ))
            },
            move |pipe: io::Cursor<Vec<u8>>, cap: usize| -> io::Result<ReaderHandle> {
                stderr_attempted_clone.store(true, Ordering::SeqCst);
                spawn_capped_reader(pipe, cap)
            },
        );

        assert!(
            result.is_err(),
            "a stdout reader thread-creation failure must propagate as Err, never panic"
        );
        assert!(
            !stderr_attempted.load(Ordering::SeqCst),
            "the stderr reader must never even be attempted once the stdout reader has already failed to spawn"
        );
    }

    #[test]
    fn spawn_reader_pair_with_joins_the_started_stdout_reader_when_stderr_reader_creation_fails() {
        let stdout_finished = Arc::new(AtomicBool::new(false));
        let stdout_finished_clone = Arc::clone(&stdout_finished);

        let result = spawn_reader_pair_with(
            Some(io::Cursor::new(Vec::<u8>::new())),
            Some(io::Cursor::new(Vec::<u8>::new())),
            1024,
            move |_pipe: io::Cursor<Vec<u8>>, _cap: usize| -> io::Result<ReaderHandle> {
                let flag = Arc::clone(&stdout_finished_clone);
                thread::Builder::new().spawn(move || {
                    // Long enough that, if `spawn_reader_pair_with` did NOT
                    // join this thread before returning, the assertion
                    // below (checked immediately after the call returns)
                    // would very likely observe `false` instead.
                    thread::sleep(Duration::from_millis(200));
                    flag.store(true, Ordering::SeqCst);
                    (Vec::new(), false)
                })
            },
            |_pipe: io::Cursor<Vec<u8>>, _cap: usize| -> io::Result<ReaderHandle> {
                Err(io::Error::other(
                    "synthetic stderr reader thread-creation failure",
                ))
            },
        );

        assert!(
            result.is_err(),
            "a stderr reader thread-creation failure must propagate as Err, never panic"
        );
        assert!(
            stdout_finished.load(Ordering::SeqCst),
            "the already-started stdout reader must be joined (waited for completion) \
             before returning the error, proving no reader thread is left running in \
             the background after a partial failure"
        );
    }

    #[test]
    fn spawn_reader_pair_still_works_normally_after_an_earlier_injected_failure() {
        // An injected failure must not leave any shared/global state
        // corrupted — a completely ordinary `spawn_reader_pair` call
        // immediately afterwards must still succeed and read real data.
        let _ = spawn_reader_pair_with(
            Some(io::Cursor::new(Vec::<u8>::new())),
            Some(io::Cursor::new(Vec::<u8>::new())),
            1024,
            |_pipe: io::Cursor<Vec<u8>>, _cap: usize| -> io::Result<ReaderHandle> {
                Err(io::Error::other("synthetic failure"))
            },
            |_pipe: io::Cursor<Vec<u8>>, _cap: usize| -> io::Result<ReaderHandle> {
                Err(io::Error::other("synthetic failure"))
            },
        );

        let (stdout_reader, stderr_reader) = spawn_reader_pair(
            Some(io::Cursor::new(b"stdout-data".to_vec())),
            Some(io::Cursor::new(b"stderr-data".to_vec())),
            1024,
        )
        .expect("spawn_reader_pair must still succeed after an unrelated earlier failure");
        let (stdout_bytes, stdout_truncated) = join_reader(stdout_reader);
        let (stderr_bytes, stderr_truncated) = join_reader(stderr_reader);
        assert_eq!(stdout_bytes, b"stdout-data");
        assert_eq!(stderr_bytes, b"stderr-data");
        assert!(!stdout_truncated);
        assert!(!stderr_truncated);
    }

    #[test]
    fn reader_thread_spawn_failure_message_is_redacted_and_reports_start_failed_not_success() {
        let root = temp_root("reader-thread-failure-message");
        let req = request("git", vec!["--version"], &root);
        let secret_bearing_error =
            io::Error::other("cannot create thread: token=abc123supersecretvalue");
        let result = finished_without_process(
            &req,
            Utc::now(),
            Instant::now(),
            ExecutionErrorKind::StartFailed,
            format!(
                "无法创建输出读取线程，已终止进程：{}",
                redact_sensitive_text(&secret_bearing_error.to_string())
            ),
        );

        assert!(
            !matches!(result.outcome, ExecutionOutcome::Success),
            "a reader-thread spawn failure must never be reported as Success"
        );
        match result.outcome {
            ExecutionOutcome::Failed { kind, detail } => {
                assert_eq!(kind, ExecutionErrorKind::StartFailed);
                assert!(
                    !detail.contains("abc123supersecretvalue"),
                    "reader-thread failure detail must not leak raw secret-looking \
                     content from the underlying io::Error: {detail}"
                );
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }
}
