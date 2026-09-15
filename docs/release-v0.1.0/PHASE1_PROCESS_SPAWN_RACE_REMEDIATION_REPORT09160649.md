# Nova v0.1.0-beta.1 Phase 1 — Process Spawn / Job Object Assignment Race Remediation

Developer: Claude Code (main developer this phase)
Independent reviewer: Codex (not yet run for this remediation)
Final acceptance: user
Generated: 2026-09-16 06:49 +0800 (Beijing time)
Repository: `D:\nova`

**This report documents remediation of the single remaining Major from Codex's re-review. It does not claim Phase 1 is approved. Phase 1 is only complete after Codex's independent re-review returns PASS. Phase 2 has not been started.**

---

## 1. Codex review reports

- `D:\nova-review\CODEX_PHASE1_REMEDIATION_REREVIEW09160610.md` — correction report, verdict FAIL, reviewed HEAD `16a30cb03a3c0fbfee50050c6ae77158f8fc7649`.
- `D:\nova-review\CODEX_PHASE1_REMEDIATION_REREVIEW09160101.md` — the full re-review it references, same verdict, same HEAD. New Blocker: 0. New Major: 1 (the spawn/assignment race, this report's entire subject). New Minor: 3 (`codex doctor --json` follow-up, generic `Unknown` frontend label, `CleanupFailed` deletion-failure test gap) — **explicitly out of scope this round** per the task instructions ("本轮只关闭这个Windows进程启动与Job Object分配竞态...不重新设计其他已关闭功能"), left untouched and unregressed.

## 2. Branch and pre-remediation HEAD

```
git rev-parse --abbrev-ref HEAD -> feature/v0.1.0-mvp
git rev-parse HEAD              -> 16a30cb03a3c0fbfee50050c6ae77158f8fc7649
git status --short --untracked-files=all       -> (no output)
git status --porcelain=v2 --untracked-files=all -> (no output)
git remote -v                                   -> (no output; no remote configured)
```
Matched exactly, confirmed against both Codex reports' own recorded state. No stop condition triggered.

## 3. Root cause of the sole remaining Major

`runtime::process::execute_blocking` (as of `16a30cb`) called `std::process::Command::spawn()` — which both creates the OS process *and* starts its initial thread running immediately — and only afterward called `AssignProcessToJobObject`. Between those two calls, the target's own code was already executing and could, in principle, spawn a descendant process of its own. Because that descendant was created *before* its parent joined the Job, Windows would not automatically make it a job member (job membership is inherited by processes spawned *after* their parent is a member, not retroactively), so it could survive a later `TerminateJobObject` call and keep running — modifying files or making network calls — after Nova reported `Cancelled`/`TimedOut`. `std::process::Command`/`Child` has no public API to create a process suspended *and* retrieve a resumable handle to its initial thread, so this race could not be closed while still spawning through `std::process::Command`.

## 4. Chosen approach: Option B (`CreateProcessW` + `CREATE_SUSPENDED`)

Option A (`std::os::windows::process::CommandExt` + `CREATE_SUSPENDED` on `std::process::Command`) was evaluated and rejected as unreliable, per this task's own stop condition ("如果std::process::Child无法可靠提供或恢复初始线程，不得用脆弱的线程枚举猜测来假装关闭问题"):

- `std::process::Command` on Windows does accept `creation_flags(CREATE_SUSPENDED)`, but `std::process::Child` exposes **no public API for the initial thread** — no thread handle accessor, no `resume()` method. The only way to resume a `Command`-spawned suspended process would be to enumerate all threads on the system (`CreateToolhelp32Snapshot`/`Thread32First`/`Thread32Next`), filter by `th32OwnerProcessID`, and hope exactly one thread is found and it is the right one. That is exactly the "fragile thread enumeration" this task explicitly forbids using to pretend the problem is closed — a process could theoretically already have more than one thread by the time enumeration runs (unlikely for a freshly `CREATE_SUSPENDED` process before resume, but not a guarantee `std::process::Command`'s public API gives us), and there is no way to obtain the *original* thread handle with the exact access rights needed for `ResumeThread` from enumeration alone without an additional `OpenThread` call carrying its own extra race/permission surface.
- Option B does not have this problem: `CreateProcessW` returns a `PROCESS_INFORMATION` struct containing the real `hThread` handle for the initial thread directly, with no enumeration, guessing, or extra permission negotiation involved.

**New module `runtime/win_spawn.rs`** implements Option B, exposing the same `program: &str` + `args: &[String]` interface the rest of the codebase already used (no shell string at any layer).

## 5. How user code is guaranteed not to run before Job assignment

The exact sequence implemented in `run_windows` (`process.rs`), matching the required order precisely:

1. **`job_object::JobObject::create()`** — creates the Job Object and sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` *before* any process exists. A failure here needs no process cleanup (nothing has been spawned yet).
2. **`win_spawn::spawn_suspended(...)`** — calls `CreateProcessW` with `CREATE_SUSPENDED`. Per the documented Win32 contract, `CREATE_SUSPENDED` means "the primary thread of the new process is suspended until the `ResumeThread` function is called" — **no code in the new process's address space executes**, including any code that could spawn a descendant, until that explicit resume. This is not an assumption Nova is trusting blindly: it is directly, empirically proven by the new `suspended_process_does_not_run_before_job_assignment_and_resume` test (§13), which spawns a real helper process suspended, and checks a marker file the helper's `main()` writes as its literal first statement — the marker does not exist immediately after creation, and still does not exist after Job assignment, only appearing after `resume()`.
3. **`job.assign_handle(child.process_handle())`** — `AssignProcessToJobObject` on the still-suspended process. If this fails, the process (which has never run any code) is terminated and reaped immediately (`handle_windows_spawn_failure`); execution never proceeds pretending containment exists when it does not.
4. **Confirmed via the `Result` of step 3** — a non-zero/success return from `AssignProcessToJobObject` is Win32's own documented success signal; there is no separate secondary "confirm" API beyond checking that return value, which the `?`-shaped `if let Err(...)` branch does.
5. **`child.resume()`** (`ResumeThread`) — only now does the target's own code begin executing, already inside the Job. If `ResumeThread` itself fails, the process (still never having run any code) is terminated and reaped the same way.
6. **Normal stdout/stderr draining and wait loop** — unchanged in spirit from before, except termination on timeout/cancel now always goes through `TerminateJobObject`, and the direct child is reaped via `SuspendedChild::wait()` afterward.

Because steps 2 and 3 both happen while the process is provably not executing any of its own code (step 2's guarantee, proven by test), and the target only starts running in step 5 — *after* job membership is already established — there is no window in which the target could spawn an uncontained descendant. This is a structural guarantee from `CREATE_SUSPENDED` semantics, not a timing race that happens to usually work.

## 6. Windows command-line argument safety

`win_spawn::quote_windows_arg` implements the documented MSVC C runtime / `CommandLineToArgvW` quoting rules directly (the same rules `std::process::Command` itself implements internally on Windows) — not a simplified approximation:

- Arguments containing no space/tab/newline/vertical-tab/quote are passed through unmodified.
- Empty arguments become `""`.
- Arguments needing quoting are wrapped in `"..."`; embedded quotes are escaped; a run of backslashes is doubled if it immediately precedes a quote (embedded or the closing one) and left literal otherwise, exactly matching the documented algorithm.
- `resolve_executable` — used to build `lpApplicationName` — resolves a bare program name (e.g. `"git"`) by searching **only `PATH` (with `PATHEXT`)**, never the current/working directory, closing the classic "planted `git.exe` in a project directory shadows the trusted one" risk; a name already containing a path separator or an absolute path is used as-is (matching `std::process::Command`'s own behavior for explicit paths). `lpApplicationName` is always set explicitly (the fully resolved path), so `CreateProcessW` never has to guess at which file to run from `lpCommandLine` alone.
- The whole target program (`&&`, `|`, `>`, `<`, `;`, parentheses, Chinese text, embedded quotes, trailing backslashes, an empty argument) is delivered to the child process as a single literal `argv` entry each — proven end to end by `argument_fidelity_round_trips_every_tricky_argument_unmodified` (§13), which round-trips 13 deliberately hostile strings through the real `nova_test_helper echo-argv` mode and asserts byte-for-byte equality per argument. No shell (`cmd.exe`, `powershell`, `sh -c`) is invoked at any point in the Windows spawn path — `CreateProcessW` is called directly against the resolved executable.
- Secrets are never written to any log: the command line built from `program`/`args` exists only in a stack-local wide-string buffer passed directly to `CreateProcessW`; it is never `Debug`-printed, and `ExecutionResult.program`/`.args` continue to be redacted through the existing `redact_sensitive_text` policy (unchanged from the previous remediation round) before being returned to any caller.

New dedicated tests for the quoting function itself: plain argument, empty argument, argument with spaces, embedded quote, literal (non-quote-adjacent) backslashes, trailing backslash before closing quote, backslashes immediately before an embedded quote, and full command-line joining — 8 tests in `win_spawn::tests`, independent of any process spawn.

## 7. stdout/stderr pipe implementation

`create_inheritable_pipe` creates each pipe via `CreatePipe` with `SECURITY_ATTRIBUTES.bInheritHandle = TRUE` (both ends initially inheritable), then immediately clears `HANDLE_FLAG_INHERIT` on the **parent's** read end via `SetHandleInformation` — only the child-side write end remains inheritable. Stdin is a freshly opened, inheritable handle to the `NUL` device (matching the previous `Stdio::null()` behavior). Inheritance into the child is further restricted explicitly via `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` (`InitializeProcThreadAttributeList`/`UpdateProcThreadAttribute` naming exactly the three handles the child needs) — the same mechanism `std::process::Command` itself uses internally on Windows to prevent unrelated inheritable handles from leaking into a child even though `bInheritHandles=TRUE` is required for the whitelist to take effect. After `CreateProcessW` returns, the parent's copies of the *write*/*stdin* ends are closed immediately (`drop(stdout_pipe.write)` etc.) — required so the read side can ever see EOF: if the parent kept its own write-end handle open, the read loop would block forever waiting for a close that would never come, even after the child itself exited. The parent's read ends are wrapped in `std::fs::File` (via `FromRawHandle`) and exposed as `Option<File>` fields on `SuspendedChild`, mirroring `std::process::Child::stdout`/`stderr` exactly so the existing `spawn_capped_reader`/size-capping/redaction code in `process.rs` needed no changes at all.

## 8. Handle lifecycle

Every `HANDLE` opened in the Windows spawn path is accounted for:

| Handle | Opened by | Closed by |
|---|---|---|
| Job object | `CreateJobObjectW` | `JobObject::Drop` (`CloseHandle`) |
| Process | `CreateProcessW` (`PROCESS_INFORMATION.hProcess`) | `SuspendedChild`'s `OwnedHandle` wrapper `Drop` |
| Thread | `CreateProcessW` (`PROCESS_INFORMATION.hThread`) | `SuspendedChild`'s `OwnedHandle` wrapper `Drop` |
| stdout pipe (both ends) | `CreatePipe` | write end: dropped explicitly right after `CreateProcessW`; read end: owned by the `std::fs::File` wrapper, closed on its `Drop` |
| stderr pipe (both ends) | `CreatePipe` | same as stdout |
| stdin (`NUL`) | `CreateFileW` | dropped explicitly right after `CreateProcessW` |
| Attribute list buffer | `InitializeProcThreadAttributeList` | `DeleteProcThreadAttributeList`, called on **every** exit path of `spawn_suspended` (success and `CreateProcessW` failure alike) |

`OwnedHandle` (a small private wrapper) centralizes the "close exactly once, never after being moved out" invariant: every handle lives inside exactly one owner at a time (no `Copy`, only explicit `.0` field reads), so there is no double-close or use-after-close path across any of the failure branches. No `unwrap()`/`panic!()` exists anywhere in `win_spawn.rs`'s non-test code — every fallible Win32 call is checked and mapped to `io::Result`.

## 9. Failure-path cleanup, all 13 required stages

| # | Stage | Handling |
|---|---|---|
| 1 | Job Object creation fails | `run_windows` returns `EarlyFailure(StartFailed)` immediately; no process was ever created, nothing to clean up. |
| 2 | Job limit-info (`SetInformationJobObject`) fails | Handled inside `JobObject::create` itself: the partially-created job handle is dropped normally (safe — closing a job with zero assigned processes is a no-op beyond deallocation) and the error propagates the same as #1. |
| 3 | stdout pipe creation fails | `spawn_suspended` returns `Err` before `CreateProcessW` is ever called; no process exists yet. |
| 4 | stderr pipe creation fails | Same as #3 (the stdout pipe's handles are dropped via `OwnedHandle`/`InheritablePipe`'s normal `Drop` when the `Result` unwinds out of the function). |
| 5 | `CreateProcessW` fails | `spawn_suspended` returns `Err`; the attribute list is deleted (`DeleteProcThreadAttributeList`) either way; all pipe/NUL handles created so far are dropped normally; `run_windows` classifies the error (`NotFound`/`PermissionDenied`/`StartFailed`) via the same `classify_spawn_error` both platforms share. |
| 6 | `AssignProcessToJobObject` fails | `handle_windows_spawn_failure` terminates and reaps the still-suspended (never-resumed, so definitely not running any user code) process; returns `StartFailed`. Proven directly by `handle_windows_spawn_failure_kills_and_reaps_a_real_suspended_process` (§13) — a real spawned suspended process, a synthetic error, and an assertion that the process is genuinely gone *and* that its marker file (proof of user code execution) never appeared. |
| 7 | `ResumeThread` fails | Same `handle_windows_spawn_failure` path — the process may be job-assigned at this point but has still never executed any of its own code, so `terminate()` is equally correct and sufficient. |
| 8 | stdout reader thread creation fails | `spawn_capped_reader`'s own `thread::spawn` failure would panic (matching the pre-existing, unmodified behavior for the non-Windows path too — this was not something this remediation round was asked to change, and `thread::spawn` failing at all is an extremely abnormal condition, typically OS resource exhaustion). |
| 9 | stderr reader thread creation fails | Same as #8. |
| 10 | Timeout | `job.terminate()` (kills the whole job — direct child and any descendant), then `child.wait()` to reap. |
| 11 | Cancel | Same as #10. |
| 12 | Normal exit | `child.try_wait()` returns `Some(status)`; no termination needed. |
| 13 | Non-zero exit | Same as #12; classified as `NonZeroExit` downstream, never shown as success. |

No path returns a success-shaped result from a failure branch. Every error string passed into `finished_without_process`/`handle_windows_spawn_failure` is redacted (`redact_sensitive_text`) before being stored. No `panic!`/`unwrap()` exists in the production (non-test) Windows spawn code.

## 10. Modified/new files

New:
- `apps/desktop/src-tauri/src/runtime/win_spawn.rs` — `CreateProcessW`/`CREATE_SUSPENDED` implementation, MSVC argv quoting, PATH-only resolution.
- `apps/desktop/src-tauri/src/bin/nova_test_helper.rs` — test-only helper binary (see §12).

Modified:
- `apps/desktop/src-tauri/src/runtime/process.rs` — `run_windows`/`run_generic` split; Windows path now spawn-suspended → job → assign → resume; non-Windows path (`run_generic`) is otherwise **unchanged in behavior** from before this round (still `std::process::Command`, still direct-child `kill()` — Nova ships Windows-only for v0.1.0, so this remains a documented, not-regressed scope boundary, unchanged from the prior round's Known Limitations).
- `apps/desktop/src-tauri/src/runtime/job_object.rs` — `assign(&self, child: &std::process::Child)` generalized to `assign_handle(&self, process_handle: HANDLE)`, decoupling it from `std::process::Child` entirely (the whole point of this remediation is assigning a process that was never a `std::process::Child` in the first place). `create()`/`terminate()`/`Drop` unchanged.
- `apps/desktop/src-tauri/src/runtime/mod.rs` — registers `win_spawn` (`#[cfg(windows)]`).
- `apps/desktop/src-tauri/Cargo.toml` — see §11.

**Not touched:** `runtime/detect.rs`, `runtime/detection_slot.rs`, `security.rs`, `commands.rs`, `lib.rs`, any frontend file (`App.vue`, `nova.ts`, `types.ts`), any product/design/release doc other than this new report — the four already-closed Majors and the machine-independence Blocker fix from the prior round required no changes for this remediation, and none were made.

## 11. Cargo features changed

```diff
 windows-sys = { version = "0.59", features = [
     "Win32_Security_Credentials",
     "Win32_Foundation",
     "Win32_System_JobObjects",
     "Win32_System_Threading",
+    "Win32_System_Pipes",
+    "Win32_Security",
+    "Win32_Storage_FileSystem",
 ] }
```

- Version unchanged (`0.59`) — no upgrade, of this or any other crate.
- `Win32_System_Pipes`: `CreatePipe`.
- `Win32_Security`: `SECURITY_ATTRIBUTES`.
- `Win32_Storage_FileSystem`: `CreateFileW` and its flag/disposition constants, used only to open `NUL` as stdin.
- `Win32_System_Threading` (already present) additionally supplies `CreateProcessW`, `STARTUPINFOEXW`, `PROCESS_INFORMATION`, `ResumeThread`, `TerminateProcess`, `WaitForSingleObject`, the `PROC_THREAD_ATTRIBUTE_*` functions/constants, and `CREATE_SUSPENDED`/`EXTENDED_STARTUPINFO_PRESENT` — no new feature needed for these, they were already covered by the flag added in the prior remediation round.
- **`git diff --stat -- Cargo.lock` is empty — zero lines changed** (verified directly, not only inferred from `cargo tree`). `cargo tree -p windows-sys@0.59.0 --invert` confirms `windows-sys 0.59.0` is already a dependency of Nova directly *and* transitively via `tauri`'s own `window-vibrancy` dependency and `tauri-build`'s `winreg`/`embed-resource` chain — enabling more of its already-resolved Win32-namespace feature flags does not pull in any new crate or version.

## 12. New tests: the `nova_test_helper` binary

Per §VIII of the task, a minimal Rust helper binary (`src/bin/nova_test_helper.rs`, no new dependencies) replaces PowerShell-timing-dependent fixtures for the two most safety-critical new tests:

- `echo-argv <args...>`: prints each argument on its own line — argv-fidelity proof.
- `marker <path>`: writes `<path>` as literally its first action, then sleeps — suspend-ordering proof.
- `grandchild-race <pid-file>`: spawns a further child of itself as literally the first statement in that branch (no `sleep`, no artificial delay of any kind for job assignment to "win" against), writes the grandchild's PID, then sleeps — immediate-grandchild race proof.
- `sleep`: idle body, used as the grandchild's own process.

It is a normal Cargo-auto-discovered `[[bin]]` target (not gated behind a feature flag), because gating it behind a non-default feature would have meant a bare `cargo test` — one of the required six verification commands — could not build it, since `env!()`/feature-gated binaries are not available to a plain `cargo test` invocation. It is never referenced from `lib.rs`/`main.rs`/any production code path, only from `#[cfg(test)]` code, and only ever touches paths explicitly passed as arguments (always inside a freshly created `std::env::temp_dir()` subdirectory in every test) or its own stdout.

**Build-timing note, and how it was resolved:** `cargo test` builds a separate *test-harness* artifact for a `[[bin]]` target (to run any `#[test]`s defined inside it, of which this one has none) — it does not reliably also produce the plain, directly-runnable executable needed to `Command::new()` it as a subprocess from another test. `helper_exe()` (in `process.rs`'s test module) checks for the plain binary first and, if missing, invokes `cargo build --bin nova_test_helper` directly (via `env!("CARGO")`), serialized across threads within the test run with `std::sync::Once` so concurrent tests never issue redundant Cargo invocations. This was verified necessary and sufficient empirically in this session: without it, six tests failed with "binary not found" even after waiting 60 seconds; with it, all tests passed reliably across three consecutive full `cargo test` runs.

**Minor build-output observation, not a functional defect:** with two `[[bin]]` targets in the package, `tauri build`'s final console line ("Built application at: ...") reports whichever binary path it printed last (observed as `nova_test_helper.exe` in this session), not necessarily `nova.exe`. Both binaries are, however, verified present and correctly built after every `npm run tauri -- build` in this session (`nova.exe` at its expected ~13.5 MB, matching prior rounds' size; `nova_test_helper.exe` a separate, unrelated ~260 KB). Since `tauri.conf.json`'s `bundle.active` remains `false`, no installer is produced regardless, so this has no current packaging impact — recorded as a Known Limitation (§16) to verify explicitly before `bundle.active` is ever turned on.

## 13. New tests: full list and what each proves

All in `runtime::process::tests` unless noted; all use `std::env::temp_dir()` fixtures, never a real user project:

- **`suspended_process_does_not_run_before_job_assignment_and_resume`** (`#[cfg(windows)]`) — the direct ordering proof from §5: marker absent after suspended creation, absent after Job assignment, present only after `resume()`.
- **`handle_windows_spawn_failure_kills_and_reaps_a_real_suspended_process`** (`#[cfg(windows)]`) — injected-failure proof for stages 6/7 of §9.
- **`immediate_grandchild_timeout_terminates_parent_and_grandchild_repeated`** (`#[cfg(windows)]`) — **20 repetitions**, `nova_test_helper grandchild-race`, 400 ms timeout, asserts `TimedOut` and that the immediately-spawned grandchild is actually gone, every iteration.
- **`immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated`** (`#[cfg(windows)]`) — same, 20 repetitions, cancel instead of timeout (cancelled ~150 ms in).
- **`argument_fidelity_round_trips_every_tricky_argument_unmodified`** — 13 fixtures (plain, empty, spaces, Chinese, embedded quotes, backslash, trailing backslash, `&&`, `|`, `>`, `;`, parens-in-path, a fake-command-looking string) via `echo-argv`, asserting exact per-line round-trip.
- **`repeated_runs_leave_no_residual_process_across_all_outcome_kinds`** (`#[cfg(windows)]`) — 5× normal success, 5× non-zero exit (unknown helper mode), 3× timeout, all via the real production path, confirming no accumulation of failures/hangs across repeated mixed-outcome runs.
- 8 pure-function tests in `win_spawn::tests` for the quoting/PATH-resolution logic (§6), independent of process spawning.

**Explicitly preserved, unmodified in assertions, and re-verified passing this round** (per §IX "不得回退"): `timeout_terminates_grandchild_process_not_just_direct_child`, `cancellation_terminates_grandchild_process_not_just_direct_child` (the original, PowerShell-based, *post-assignment* grandchild tests — kept because they still validate normal Job Object containment behavior, distinct from the immediate-race tests above), `argument_containing_shell_metacharacters_is_not_reinterpreted`, `output_over_cap_is_truncated_not_unbounded`, `program_and_args_are_redacted_in_the_result`, `rejects_execution_outside_allowed_root_without_spawning`, `missing_executable_is_reported_as_not_found`, `git_version_succeeds_and_captures_stdout`, `nonzero_exit_is_captured_with_separated_stderr`, all `runtime::detect::tests::*` (CLI classification, directory probe), all `runtime::detection_slot::tests::*` (concurrency), all `security::tests::*` (redaction), all `runtime::path_guard::tests::*` (Chinese/space paths, traversal, junction escape).

## 14. Immediate-grandchild repeated-run results

20/20 passed for timeout; 20/20 passed for cancel — zero failures across both, run three separate times total in this session (once during initial development, twice more during the final verification pass in §15) with no flakiness observed.

## 15. Argument fidelity test result

Passed: all 13 fixture arguments (including `""`, embedded Chinese, embedded double quotes, a lone trailing backslash, and four distinct shell-metacharacter-looking strings) were received by the real spawned helper process as exactly the original strings, one per line, in original order — verified by exact string equality per argument, not a substring/contains check.

## 16. Six full verification commands — exact results

| # | Command | Working dir | Exit | Result |
|---|---|---|---|---|
| 1 | `cargo fmt --check` | `apps/desktop/src-tauri` | 0 | PASS |
| 2 | `cargo clippy --all-targets --all-features -- -D warnings` | `apps/desktop/src-tauri` | 0 | PASS — 0 warnings (fixed, not suppressed: 3×`manual_repeat_n`, 2×`unnecessary_mut_passed`, 1×`zombie_processes` on the deliberately-not-waited test-fixture grandchild, annotated with `#[allow(clippy::zombie_processes)]` and an explanatory comment since waiting on it would defeat the fixture's purpose) |
| 3 | `cargo test` | `apps/desktop/src-tauri` | 0 | PASS — **109 passed, 0 failed** (up from 93; run three times total this session, stable every time; timing breakdown: ~5-9s once the helper binary is already built, one-time ~23-34s on a cold run needing to build it) |
| 4 | `npm test` | `apps/desktop` | 0 | PASS — 13 passed, unchanged (this round touched no frontend file) |
| 5 | `npm run build` | `apps/desktop` | 0 | PASS — type check + production bundle, unchanged output |
| 6 | `npm run tauri -- build` | `apps/desktop` | 0 | PASS — release compile ~1m 14s; both `nova.exe` and `nova_test_helper.exe` built and verified present (see §12's note on the cosmetic "Built application at" log line) |

## 17. Rust test total

**109** (up from 93 before this round). 16 new tests: 6 in `runtime::process::tests` (suspend-ordering proof, injected spawn-failure cleanup, 2× repeated immediate-grandchild race ×20 each, argv fidelity, repeated-run residual-process check) + 10 in `runtime::win_spawn::tests` (8 quoting/command-line tests + 2 `resolve_executable` tests).

## 18. Skips

**None.** Every new Windows-specific test is gated with `#[cfg(windows)]` (compiled out, not skipped-at-runtime, on non-Windows targets — consistent with how the prior round's grandchild tests were already gated, and with Nova's actual Windows-only v0.1.0 target). No test uses `#[ignore]`. The one environment-dependent test from the prior round (`path_guard::rejects_directory_junction_that_escapes_root`, needing `mklink /J`) is unchanged and, as in every previous run this week, executed for real rather than skipping.

## 19. Known limitations

- **`tauri build`'s final "Built application at" log line is not reliably `nova.exe`** once a second `[[bin]]` target exists in the package (§12) — purely cosmetic in Phase 1 (`bundle.active: false`, no installer produced), but should be explicitly verified (and `tauri.conf.json` configured with an explicit main-binary reference if Tauri's own docs recommend one for multi-binary packages) before `bundle.active` is ever turned on for a real release build.
- **`nova_test_helper` is compiled into every `cargo build`/`npm run tauri -- build` output**, not gated behind a feature (§12 explains why: a feature gate would break the required plain `cargo test`). It is small (~260 KB) and never referenced by production code, but it is a real extra artifact in `target/release/`.
- Everything already listed as a Known Limitation in the two prior Phase 1 reports and not touched this round still applies unchanged: the `--help`-based Codex/Claude status probe placeholder, the `CleanupFailed` deletion-failure automated-test gap, the small residual spawn-to-assign race this exact report closes having existed, uppercase bare-token-prefix redaction, full URL-encoded-delimiter redaction, and non-Windows process-tree containment not being guaranteed (still explicitly out of scope — Nova ships Windows-only for v0.1.0).

## 20. Remediation commit SHA

**Deliberately not written in this file**, for the same self-reference reason as both prior reports: committed in the same single commit as the code it describes. The authoritative SHA is `git log -1 --format=%H` on `feature/v0.1.0-mvp` immediately after this commit, reported in the developer's final reply to the user.

## 21. Rollback method

```
git revert <this remediation commit SHA> --no-edit
```
Returns the branch to `16a30cb03a3c0fbfee50050c6ae77158f8fc7649` (the previous remediation, PARTIALLY CLOSED per Codex). `git reset --hard` is not used. No tag exists for this milestone yet.

## 22. git remote status

```
git remote -v -> (no output)
```
Unchanged, not added.

## 23. Phase 2 has not started

**Explicit statement, unchanged from both prior reports: no SQLite schema/migration work, no task state model, no project-entry flow, no GitHub import, no template creation, no real AI workflow, no fourth-edition UI.** This round's entire diff is confined to the single Windows process-launch containment race, its Cargo feature dependency, its test helper binary, its tests, and this report.

## 24. Codex re-review material checklist

1. This report and the remediation commit it is part of (single commit on `feature/v0.1.0-mvp`, parent `16a30cb03a3c0fbfee50050c6ae77158f8fc7649`).
2. `git diff 16a30cb03a3c0fbfee50050c6ae77158f8fc7649 <commit> --stat` — confirms the file list in §10 exactly, and that `runtime/detect.rs`, `security.rs`, `commands.rs`, `lib.rs`, and every frontend file are absent from the diff.
3. `git diff 16a30cb03a3c0fbfee50050c6ae77158f8fc7649 <commit> -- Cargo.lock` — expected empty.
4. `apps/desktop/src-tauri/src/runtime/win_spawn.rs` — the `CreateProcessW`/`CREATE_SUSPENDED` implementation itself; specifically confirm the call order in `run_windows` (`process.rs`) matches §5's required sequence.
5. `cargo test runtime::process::tests::suspended_process_does_not_run_before_job_assignment_and_resume -- --exact --nocapture` — the direct ordering proof.
6. `cargo test runtime::process::tests::immediate_grandchild -- --nocapture` — both 20-repetition race tests.
7. `cargo test runtime::process::tests::argument_fidelity -- --nocapture` and `cargo test runtime::win_spawn:: -- --nocapture` — argv safety.
8. `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `npm test`, `npm run build`, `npm run tauri -- build` — rerun independently; expected exit 0 for all six, 109 Rust tests passing.
9. `D:\nova-review\CODEX_PHASE1_REMEDIATION_REREVIEW09160610.md` and `...09160101.md` — the FAIL reviews this remediation responds to.
10. Post-commit `git status --short --untracked-files=all` and `git status --porcelain=v2 --untracked-files=all` — both expected empty.
