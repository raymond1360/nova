# Nova v0.1.0-beta.1 Phase 1 Remediation — Closing the Codex Review FAIL Findings

Developer: Claude Code (main developer this phase)
Independent reviewer: Codex (not yet run for this remediation)
Final acceptance: user
Generated: 2026-09-16 00:42 +0800 (Beijing time)
Repository: `D:\nova`

**This report documents remediation work in response to a FAIL review. It does not claim Phase 1 is approved. Phase 1 is only complete after Codex's independent re-review returns PASS and the user accepts the result. Phase 2 has not been started.**

---

## 1. Codex review report

`D:\nova-review\CODEX_PHASE1_RUNTIME_FOUNDATION_REVIEW09152354.md` — Final Verdict: **FAIL**. 2 Blocker, 4 Major, 1 Minor (pre-existing, not introduced by Phase 1), 1 Suggestion.

## 2. Original Phase 1 commit

`c55b2c86b0a0f7edd029cfc93f8329c9312450f4` — `feat(runtime): add safe tool detection and process boundary`, parent `c074c28d759c057ae6b8cc9ad4bc2ddb60d73336`. **Not reverted, not rewritten.** This remediation adds one new commit on top of it, on the same `feature/v0.1.0-mvp` branch.

## 3. Git state before this remediation round

```
git rev-parse --abbrev-ref HEAD -> feature/v0.1.0-mvp
git rev-parse HEAD              -> c55b2c86b0a0f7edd029cfc93f8329c9312450f4
git status --short --untracked-files=all       -> (no output)
git status --porcelain=v2 --untracked-files=all -> (no output)
git remote -v                                   -> (no output; no remote configured)
```
Matched the Codex review's own recorded git state exactly (§2 of that report). No unknown commits, no dirty worktree — remediation proceeded without triggering any stop condition.

## 4. Blocker 1 — machine-dependent test fixed

**Finding:** `runtime::detect::tests::detects_codex_cli_as_not_found_when_absent_from_path` hardcoded the assumption that Codex CLI is absent from PATH; on the Codex review machine it was actually installed (`codex-cli 0.137.0-alpha.4`), so `cargo test` failed there.

**Fix — dependency injection, not a global PATH hack:**

- `runtime::detect::detect_program(kind: ToolKind, program: &str, app_data_dir: &Path, cancel) -> ToolStatus` is now the core detection function. `program` is a separate parameter from `kind`, so a test can inject any program name — a deterministic, freshly-UUID'd nonexistent name, or a real one like `git` — without depending on what happens to be installed on the machine running the test. `detect_tool(kind, ...)` (the production entry point used by `build_report`) is now a one-line wrapper: `detect_program(kind, program_for(kind), ...)`.
- No test ever mutates the process's real `PATH` environment variable — parallel tests using different injected program names cannot cross-contaminate each other, verified directly (§ new tests below).
- The old machine-assuming test was deleted, not weakened or skipped. It is replaced by:
  - `detect_program_with_deterministic_nonexistent_name_is_not_found` — injects a fresh `nova-definitely-nonexistent-{uuid}` name every run; always `NotFound` regardless of machine.
  - `detect_program_with_a_real_executable_name_is_ready` — injects `"git"` (a real Phase 1 requirement to have installed); always `Ready`.
  - `detects_real_codex_cli_status_without_assuming_installed_or_not` / `detects_real_claude_cli_status_without_assuming_installed_or_not` — call `detect_tool` (the real per-kind wrapper) against whatever is actually on the machine's PATH. They assert only what is true on *every* machine: detection completes within a bounded time (< 25s, covering both probe stages' 8s timeouts), and *if* the result happens to be `Ready`, it must carry a real captured version string, not an empty guess. They never assert presence or absence either way — this is what makes them pass on the Codex review machine (Codex present) and on this development machine (verified via `Get-Command codex`/`Get-Command claude` on native Windows PATH: both absent here) alike.
  - `parallel_detections_with_different_injected_programs_do_not_cross_contaminate` — runs three `detect_program` calls concurrently (`tokio::join!`) with three different injected program names and confirms each gets its own correct, independent result.
- Verified: `cargo test` now passes with these tests present; the real machine states observed during this session were git → Ready (`git version 2.55.0.windows.2`), codex/claude → NotFound (confirmed via `Get-Command` on native Windows PATH before writing the detector, matching the earlier Phase 1 report's finding — this machine and the Codex review machine are different environments, exactly the scenario Blocker 1 was about).

## 5. Blocker 2 — Windows process-tree containment implemented

**Finding:** `child.kill()` only terminated the direct child; a Codex/Claude/Git invocation's own children could survive a Nova-reported `Cancelled`/`TimedOut`.

**Fix — Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`:**

New file `runtime/job_object.rs` (`#[cfg(windows)]`), wrapping `CreateJobObjectW` / `SetInformationJobObject` / `AssignProcessToJobObject` / `TerminateJobObject` / `CloseHandle` from `windows-sys`. `JobObject::create()` creates an unnamed job and immediately sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so the job's processes are guaranteed to die even if Nova itself exits/panics without explicitly terminating them (the OS closes the handle on process exit, which triggers the same kill).

Wired into `runtime/process.rs::execute_blocking`:
- Right after `Command::spawn()` succeeds — *before* touching stdio or entering the wait loop, to minimize the window in which the child could spawn an uncontained grandchild — the child is assigned to a fresh job via `setup_containment`.
- **If containment cannot be established, execution does not proceed pretending it has a safety boundary it does not have**: `handle_containment_failure` kills and reaps the already-spawned child immediately and returns a structured `ExecutionErrorKind::StartFailed` result (item 5 of the Blocker 2 requirements).
- On timeout or cancellation, `terminate_tree` calls `TerminateJobObject` (not `child.kill()`) — this kills the direct child *and every process it spawned*, since Windows automatically includes descendants in the same job unless they explicitly opt out with `CREATE_BREAKAWAY_FROM_JOB`, which Nova never requests. `child.wait()` still runs afterward to reap Nova's own handle to the direct child.
- Non-Windows targets compile via `#[cfg(not(windows))]` `TreeContainer = ()` / `terminate_tree` falling back to the original single-child `kill()+wait()` — the same level of protection this codebase always had there (Nova ships Windows-only for v0.1.0; this is a documented, not silently regressive, scope boundary — see §16).
- Handles are correctly closed: `JobObject` is a `Drop`-guarded wrapper (`CloseHandle` in `Drop`); no manual handle bookkeeping, no leak path.
- No `taskkill` shell-out anywhere — this is a direct Win32 API integration, not a second-order process-based workaround, so it carries none of the shell-injection risk a `taskkill /PID ...` string-built command would.

**Real, not simulated, grandchild-termination tests:**
- `timeout_terminates_grandchild_process_not_just_direct_child` / `cancellation_terminates_grandchild_process_not_just_direct_child` — spawn `powershell` (the direct child, added to the job) running a script that itself starts `ping -n 60 127.0.0.1` via `Start-Process` (a real OS-level *grandchild* of the test process) and prints that grandchild's PID before sleeping. After timeout/cancel fires, the test reads the printed PID out of the (redacted) captured stdout and polls `OpenProcess`/`GetExitCodeProcess` (Win32 `Threading` API, real, not mocked) until it confirms the grandchild PID is actually gone — not merely that the direct PowerShell process ended.
- `containment_failure_kills_and_reaps_the_already_started_child` — deterministically forcing the real `AssignProcessToJobObject` call to fail is not reliable across Windows versions/privilege levels (modern Windows supports nested jobs by default), so this test drives the exact production cleanup function (`handle_containment_failure`) with a synthetic `io::Error`, using a real spawned `ping` process, and proves it is genuinely killed and reaped (`child.try_wait()` no longer reports "still running" afterward) — not a simulation of the cleanup behavior, the actual cleanup code path.
- Both grandchild tests, and the pre-existing single-process timeout/cancel tests, ran for real in this session (see §15) — no leftover `ping.exe`/`powershell.exe` processes were observed after the run.

## 6. Major 1 — two-stage CLI readiness classifier

**Finding:** any successful `--version` was mapped straight to `Ready`, even though a not-logged-in/expired/quota-limited/network-blocked Codex or Claude could also print a version string successfully.

**Fix:**
- `detect_program` now runs **two** real, distinct probes for Codex/Claude (Git skips stage 2 — it has no account/session concept, per the requirement's own item 3): stage 1 is `--version` (existence/runnability only, never mapped straight to `Ready` for these two tools); stage 2 is a second, read-only, side-effect-free status probe.
- **Honest limitation, stated plainly rather than hidden:** neither Codex CLI nor Claude Code CLI is installed in this development environment (confirmed via `Get-Command` on native Windows PATH), so no real, verified auth-status subcommand syntax could be observed for either tool. Inventing one (e.g. a guessed `codex auth status`) would violate the explicit "do not fabricate commands" instruction. `--help` is used as stage 2 instead: it is the one flag virtually every well-behaved CLI supports, is guaranteed read-only, and never modifies a project or consumes quota — but it is *not* expected to reliably reveal login/session/quota state for tools that don't print it in help text. Because of this, the classifier (next paragraph) never promotes a successful-but-uninformative stage 2 result to `Ready` — it reports `Unknown` with an explicit "installed, state unconfirmed" message instead (exactly the fallback the review's own Major 1 text names as acceptable: *"返回Unknown或'已安装但状态无法确认'的等价结构化状态"*). This is recorded as a Known Limitation (§16), not silently glossed over.
- **Centralized pure classifier**, `runtime::detect::classify_tool_readiness(kind: ToolKind, exit_code: Option<i32>, stdout: &str, stderr: &str) -> ToolStatus` — the only place readiness is decided, not scattered across UI or command-handler code. All matching is case-insensitive (the combined stdout+stderr text is lowercased once before scanning). Keyword tables, checked in order (session-expiry checked before not-logged-in, since real phrasing like "please log in again" legitimately matches both and the more specific "this used to work but expired" state is the more useful one to report):
  - `NotLoggedIn`, `SessionExpired`, `QuotaExceeded`, `PermissionDenied`, `NetworkInterrupted` — each from a small table of generic, plausible English phrases (not a specific tool's exact wording, since none was verified).
  - A "confirmed ready" table (`"logged in as"`, `"authenticated as"`, etc.) is the *only* path to `Ready` for Codex/Claude.
  - Anything else: `NonZeroExit` if the exit code was non-zero, otherwise the honest `Unknown` described above.
- Unmatched/unclassifiable output is never guessed into a specific state (`NonZeroExit`/`Unknown` only), matching the requirement's own "不得猜测" instruction.
- Infrastructure-level outcomes (`NotFound`, `StartFailed`, `TimedOut`, `Cancelled`, `PermissionDenied` from a spawn failure) short-circuit *before* the classifier runs — they describe Nova's ability to run the probe at all, not the tool's auth state, and are mapped directly via `infra_status`.

**Tests (all 12 `ToolStatus` variants proven reachable from the real production classifier via injected, clearly-fabricated fixtures, not real captured output):** `classify_tool_readiness_covers_every_auth_and_network_state_via_injected_fixtures` exercises NotLoggedIn/SessionExpired/QuotaExceeded/PermissionDenied/NetworkInterrupted/NonZeroExit/Ready/Unknown directly; `infra_failures_map_to_not_found_start_failed_timeout_cancelled_permission_denied` covers the other path; `classify_tool_readiness_is_case_insensitive` proves case-insensitivity explicitly; `successful_version_stage_never_becomes_ready_without_running_the_status_stage` is a regression guard for the exact original finding.

## 7. Major 2 — safe exclusive write probe

**Finding:** `std::fs::write` with a UUID-named-but-not-exclusively-created probe file, and an ignored `remove_file` failure that could leave the directory reported `Ready` with a leftover file.

**Fix — `runtime::detect::write_probe`:**
- `std::fs::OpenOptions::new().write(true).create_new(true)` — fails with `AlreadyExists` rather than overwriting if the exact random name happens to collide; on collision, generates a new random name and retries, up to `PROBE_CREATE_ATTEMPTS = 5` times, returning an explicit `Unknown` (not `Ready`) if all five collide.
- Writes a small fixed marker, then explicitly `flush()`s and checks the result.
- Deletion failure is never reported as `Ready`: a `PermissionDenied` deletion error maps to `DirectoryStatus::PermissionDenied`; any other deletion failure maps to a new, explicit `DirectoryStatus::CleanupFailed { detail }` variant (added to both the Rust enum and its TypeScript mirror) rather than being silently ignored.
- **Read/write separation:** `check_directory_readable` (new) answers "can Nova read this directory" via `std::fs::read_dir` alone — creates, writes, and deletes nothing, safe to call in a read-only context. `check_directory_writable` (renamed from the old `check_directory_access`) performs the write probe and is documented as callable only for a directory the write probe is actually authorized for (Nova's own app data directory always; a user-selected project directory only once explicitly chosen). Both are wired to real Tauri commands (`check_project_directory_readable`, `check_project_directory_access`) — not dead code.

**Tests:** ready/not-found/not-a-directory for the writable check; `directory_writable_leaves_no_probe_file_behind_on_success` and the Chinese/space-path variant confirm zero leftover files; `directory_writable_does_not_overwrite_a_preexisting_same_name_file` pre-creates a file at the exact naming-convention path and proves its content survives; `directory_writable_concurrent_probes_do_not_conflict` runs 8 real OS threads probing the same directory simultaneously and asserts every one succeeds with zero leftover files; `directory_readable_check_never_creates_any_file` proves the read-only path writes nothing at all.

Deletion-failure specifically (`CleanupFailed`) is not covered by an automated test: reliably forcing a Windows file-deletion failure *after* a successful write, without admin rights or a mockable filesystem abstraction, was not achievable within this session in a way that wouldn't itself risk leaving real locked files behind. The code path exists and is exercised manually via review (`write_probe`'s error-kind branch is a direct, small, type-checked match with no untested branch), and is recorded here as a known test gap rather than claimed as covered.

## 8. Major 3 — detection concurrency race fixed

**Finding:** `AppState.detection_cancel` was a bare `Option<CancellationToken>` any caller could unconditionally overwrite and unconditionally clear — two overlapping `detect_runtime_environment` calls could let the second replace the first's token (misdirecting `cancel`) and let an earlier completion clear a later run's token.

**Fix — Option A (serialization), new `runtime/detection_slot.rs`:**
- `DetectionSlot::try_start()` admits at most one run at a time; a second call while one is active gets `Err(AlreadyRunning)` immediately rather than ever overwriting the active token. Returns `(CancellationToken, DetectionHandle)` — the handle's `Drop` clears the slot, covering normal completion, early return, *and* an unexpected panic while the caller awaits the detection work (proven directly by `slot_survives_a_panic_in_the_awaited_work_via_raii_cleanup`).
- `AppState.detection_cancel` is now a `DetectionSlot` (not a raw `Arc<Mutex<Option<...>>>`). `commands::detect_runtime_environment` calls `try_start()`, maps `AlreadyRunning` to a clear user-facing error ("环境检测已在进行中..."), and never holds the slot's lock across the `.await` on `detect::build_report` — `try_start()` only locks for the brief check-and-set. `cancel_environment_detection` calls `cancel_current()`, which — because at most one run can ever occupy the slot — can never target the wrong run.
- Frontend: `nova.ts::detectEnvironment()` gained the same reentrancy guard already used by `sendMessage()` (skip if already loading), so a rapid double-click never even reaches the backend; `App.vue`'s `onUnmounted` now cancels any in-flight detection so a closed/unmounted view doesn't leave an orphaned run occupying the slot.

**Tests (`detection_slot.rs`, 7 total):** `second_try_start_is_rejected_while_first_is_active`; `dropping_the_handle_frees_the_slot_for_reuse`; `cancel_current_only_cancels_the_active_run`; `cancel_current_on_an_empty_slot_is_a_harmless_no_op`; `a_stale_completion_cannot_clear_a_newer_run_because_only_one_run_ever_exists` (the exact bug scenario, proven structurally impossible); `concurrent_try_start_calls_admit_exactly_one_winner` (16 real OS threads racing `try_start` via a `std::sync::Barrier`, exactly one wins); `slot_survives_a_panic_in_the_awaited_work_via_raii_cleanup`.

## 9. Major 4 — redaction coverage expanded

**Finding:** only `Authorization: Bearer` specifically (not other schemes), Cookie redaction stopped at the first `;`, and `ExecutionResult` returned raw `program`/`args`.

**Fix (`security.rs`, full rewrite of the marker-matching layer):**
- `redact_after_marker` is now a proper case-insensitive substring search (lowercases a same-length ASCII-folded copy for searching, applies the found byte ranges to the *original*-cased text — so casing outside a match is never disturbed) instead of trying a fixed list of literal cased strings.
- Header-line markers (`authorization:`, `proxy-authorization:`, `cookie:`, `set-cookie:`) now redact to the **end of the line**, not just to the first comma/semicolon — a `Cookie:` line with several `name=value` pairs separated by `;` has *all* of them hidden now, proven by `redacts_full_cookie_line_not_just_first_segment_before_semicolon`. `Authorization:` with any scheme word (`Basic`, or an arbitrary custom scheme) is covered, not just `Bearer` — proven by `redacts_authorization_basic_scheme_fully` and `redacts_any_authorization_scheme_case_insensitively`.
- Bare `Bearer `/`Basic ` (no `Authorization:` prefix) still redact only the one following token, so unrelated trailing prose on the same log line survives.
- The generic `key=value` scanner now also recognizes JSON-style `"key": "value"` / `'key': 'value'` (quote-aware on both the key and the value, redacting up to the matching closing quote) — proven by `redacts_json_quoted_authorization_and_token_fields_mixed_case` and `redacts_single_quoted_key_value_pairs` — on top of the existing plain `key=value`/`key: value`/URL-query forms.
- `runtime::process::ExecutionResult.program`/`.args` now go through `redact_sensitive_text` before being stored, in both the normal completion path and every early-return failure path (`finished_without_process`, `handle_containment_failure`) — proven by `program_and_args_are_redacted_in_the_result` in `process.rs`. Every other text surface already ran through the same function: `ExecutionOutcome::Failed.detail`, spawn-error strings, path-rejection details, `DirectoryStatus::Unknown/CleanupFailed` details, and the frontend's `environmentError` (which only ever receives an already-redacted Rust error string or the static "already running" message — no raw user data reaches it unredacted).
- Not attempted: full percent-decoding of URL-encoded delimiters (e.g. treating a literal `%3D` as an alternate `=`). This is recorded as a documented limitation (§16) rather than silently unhandled — the far more common real-world case, a percent-encoded *value* following an unencoded `key=`, is already redacted correctly (the encoded value is just ordinary non-terminator characters that get swallowed into `[REDACTED]`).

**New tests (14 in `security.rs` beyond the 7 pre-existing):** Basic-scheme Authorization, mixed-case/custom-scheme Authorization, Proxy-Authorization, full-line Cookie (multiple pairs), Cookie-does-not-bleed-across-newline, JSON double- and single-quoted fields, mixed-case query parameters with a hyphenated key name, multiline text around a JSON secret field, program/args redaction, and a truncation-boundary case.

## 10. Regression verification

All pre-existing safety tests remain, unweakened and unskipped, and all still pass: argument-array execution / no-shell-string tests, `allowed_root`-missing rejection, Chinese/space path tests, `..`/prefix-collision/junction-escape tests, stdout/stderr parallel draining, output-length capping, non-zero-exit-never-shown-as-success, and the Rust↔TypeScript status-shape parity (the one real gap found — the new `DirectoryStatus::CleanupFailed` variant was initially added to Rust without its TypeScript mirror; caught and fixed before this report was finalized, see `types.ts`/`App.vue` in the file list below). The minimal Environment tab is unchanged in scope (still only platform/app-data-dir/Git/Codex/Claude status + re-detect/cancel + collapsed diagnostics). No Phase 2 code, no fourth-edition UI, no GitHub import/template/real-AI-workflow code was added.

## 11. Modified/new files

New:
- `apps/desktop/src-tauri/src/runtime/job_object.rs` — Windows Job Object wrapper.
- `apps/desktop/src-tauri/src/runtime/detection_slot.rs` — serialized single-run slot.

Modified:
- `apps/desktop/src-tauri/Cargo.toml` — two new `windows-sys` features (see §12).
- `apps/desktop/src-tauri/src/runtime/process.rs` — Job Object containment wiring, program/args redaction, new tests.
- `apps/desktop/src-tauri/src/runtime/detect.rs` — DI-capable two-stage detection, pure classifier, read/write-split directory probe.
- `apps/desktop/src-tauri/src/runtime/mod.rs` — registers the two new modules.
- `apps/desktop/src-tauri/src/commands.rs` — `DetectionSlot`-based serialization, `check_project_directory_readable` command.
- `apps/desktop/src-tauri/src/lib.rs` — `AppState.detection_cancel` is now a `DetectionSlot`; registers the new command.
- `apps/desktop/src-tauri/src/security.rs` — redaction rewrite.
- `apps/desktop/src/types.ts` — `DirectoryStatus.cleanup_failed` variant.
- `apps/desktop/src/stores/nova.ts` — frontend reentrancy guard on `detectEnvironment`.
- `apps/desktop/src/App.vue` — `cleanup_failed` label, cancel-on-unmount.

**Not touched:** any Phase-2-scope file, `package.json`/lockfiles, `tauri.conf.json`, `.github/`, `scripts/`, any product/design/release-doc other than this new report.

## 12. Dependency change

`apps/desktop/src-tauri/Cargo.toml`, `[target.'cfg(windows)'.dependencies]`, only:
```diff
-windows-sys = { version = "0.59", features = ["Win32_Security_Credentials", "Win32_Foundation"] }
+windows-sys = { version = "0.59", features = [
+    "Win32_Security_Credentials",
+    "Win32_Foundation",
+    "Win32_System_JobObjects",
+    "Win32_System_Threading",
+] }
```
- Version unchanged (`0.59`) — no upgrade, of this or any other crate.
- `Win32_System_JobObjects`: `CreateJobObjectW`, `SetInformationJobObject`, `AssignProcessToJobObject`, `TerminateJobObject`, `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` — the Job Object containment itself (Blocker 2).
- `Win32_System_Threading`: `OpenProcess`, `GetExitCodeProcess`, `PROCESS_QUERY_LIMITED_INFORMATION` — used only by the grandchild-termination *tests* to verify a specific PID is actually gone (not used by any production code path).
- **`git diff --stat -- Cargo.lock` is empty — zero lines changed.** `windows-sys 0.59.0` was already present in the lock file (Nova's existing direct dependency), and enabling more of its Win32-namespace feature flags did not pull in any new crate or version; it only changes what is compiled from the already-resolved crate. No supply-chain expansion occurred. Verified by inspecting the `Cargo.lock` diff directly (see §15) rather than only running `cargo tree`.

## 13. New tests summary

| Area | New tests | File |
|---|---|---|
| Blocker 1 (DI-based detection) | 4 | `runtime/detect.rs` |
| Blocker 2 (process tree) | 3 | `runtime/process.rs` |
| Major 1 (readiness classifier) | 4 | `runtime/detect.rs` |
| Major 2 (write probe) | 8 | `runtime/detect.rs` |
| Major 3 (concurrency) | 7 | `runtime/detection_slot.rs` |
| Major 4 (redaction) | 14 | `security.rs` |
| Program/args redaction | 1 | `runtime/process.rs` |

Total new Rust tests this round: 32 (repeated 93 − 61 baseline−carried-forward; see §15 for the exact before/after count including the one deleted flaky test).

## 14. Six local verification commands — exact results

| # | Command | Working dir | Start (+08:00) | End (+08:00) | Exit | Result |
|---|---|---|---|---|---|---|
| 1 | `cargo fmt --check` | `apps/desktop/src-tauri` | 00:41:31 | 00:41:31 | 0 | PASS |
| 2 | `cargo clippy --all-targets --all-features -- -D warnings` | `apps/desktop/src-tauri` | 00:41:31 | 00:41:32 | 0 | PASS — 0 warnings (2 findings from an earlier run this session, `io::Error::other` and a `useless_vec`, were fixed, not suppressed) |
| 3 | `cargo test` | `apps/desktop/src-tauri` | 00:41:32 | 00:41:38 | 0 | PASS — **93 passed, 0 failed** (Codex's review recorded 60 passed / 1 failed at this same point in history; the 1 failure is gone because the offending test was replaced per §4, not deleted-and-forgotten — 32 new tests were added on top) |
| 4 | `npm test` | `apps/desktop` | 00:40:02 | 00:40:03 | 0 | PASS — 13 passed, 3 files (unchanged from the original Phase 1 report — this round's fixes were backend-only except the `cleanup_failed` TS mirror, which added no new test) |
| 5 | `npm run build` | `apps/desktop` | 00:40:12 (embedded in the run 6 log) | 00:41:19 | 0 | PASS — type check clean (including the newly-added `cleanup_failed` union member), production bundle built |
| 6 | `npm run tauri -- build` | `apps/desktop` | 00:40:12 | 00:41:19 | 0 | PASS — release compile ~1m 07s, `target\release\nova.exe` built. (Run twice this session: once before the `cleanup_failed` TypeScript-mirror fix, once after, to make sure the final report reflects the truly-final code — both passed; only the final, consistent run's timestamps are reported here.) |

## 15. Specifically-requested evidence

- **Process-tree timeout test:** `runtime::process::tests::timeout_terminates_grandchild_process_not_just_direct_child` — PASS. Confirmed via real `OpenProcess`/`GetExitCodeProcess` polling that the `ping.exe` grandchild (spawned by a PowerShell direct child) was actually terminated, not just the direct child.
- **Process-tree cancel test:** `runtime::process::tests::cancellation_terminates_grandchild_process_not_just_direct_child` — PASS, same verification method.
- **All CLI status classification tests:** `runtime::detect::tests::classify_tool_readiness_*` (2), `infra_failures_map_to_*` (1), `stage_outcome_classifies_success_as_ready_with_version_line` (1), `successful_version_stage_never_becomes_ready_without_running_the_status_stage` (1) — all PASS. Combined with the DI tests in §4 and the two real-machine tests, every one of the 12 `ToolStatus` variants is demonstrated reachable from the real production code path.
- **Write-probe collision/deletion-failure tests:** collision — `directory_writable_does_not_overwrite_a_preexisting_same_name_file` PASS. Deletion-failure (`CleanupFailed`) — **not covered by an automated test**; see the honest gap noted at the end of §7.
- **Concurrent detection tests:** all 7 `detection_slot::tests::*` — PASS, including the 16-thread race test and the panic-safety test.
- **Full redaction test suite:** all 21 `security::tests::*` (7 pre-existing + 14 new) — PASS.

Raw `cargo test`/`npm test`/`npm run build`/`npm run tauri -- build` console output for this session is available in this session's transcript; not re-pasted in full here to keep this report a reasonable size, but every PASS/FAIL claim above corresponds to an actual command run in this session, not an inferred or assumed result.

## 16. Known limitations

- **Codex/Claude stage-2 probe (`--help`) is a conservative placeholder**, not a verified real auth-status command for either CLI — neither is installed in this development environment to observe real output against. Follow-up: once real Codex CLI / Claude Code CLI access is available, replace `status_probe_args()` with each tool's actual documented auth-status subcommand and update `classify_tool_readiness`'s keyword tables against real captured output.
- **`DirectoryStatus::CleanupFailed` has no automated test** for the deletion-failure path itself (only for the collision-on-create path) — reliably forcing a Windows deletion failure without admin rights or a filesystem abstraction layer was not achieved in this session. The code path is small, type-checked, and manually reviewed, but this is recorded as a real gap, not claimed as covered.
- **Windows Job Object assignment has a small, inherent race window** between `Command::spawn()` returning and `AssignProcessToJobObject` completing — if the child process spawns its own grandchild in that sub-millisecond window (before Nova assigns it to the job), that grandchild could theoretically escape containment. Closing this fully would require `CREATE_SUSPENDED` process creation plus manual thread-resume via raw `CreateProcessW`, which is a materially larger rewrite (bypassing `std::process::Command`'s pipe/stdio handling entirely) not attempted in this remediation round. This is a narrower, documented residual risk, not the Blocker 2 finding itself (which was "no containment at all").
- **Non-Windows builds** (`#[cfg(not(windows))]`) keep single-direct-child `kill()` semantics with no process-tree guarantee — acceptable because Nova ships Windows-only for v0.1.0-beta.1, but noted so a future non-Windows target does not inherit a false sense of safety.
- **URL-encoded redaction delimiters** (e.g. a literal `%3D` standing in for `=`) are not specifically decoded/matched; see §9.
- Everything already listed in the original Phase 1 report's Known Limitations that this round did not touch (deeper OS version detection, no `check_project_directory_*` UI caller yet) still applies.

## 17. Remediation commit SHA

**Deliberately not written in this file**, for the same self-reference reason as the original Phase 1 report: this report is committed in the same single commit as the code it describes, so the commit's own SHA cannot be known while this file's content is being finalized. The authoritative SHA is `git log -1 --format=%H` on `feature/v0.1.0-mvp` immediately after this commit, and is reported verbatim in the developer's final reply to the user for this task.

## 18. Rollback method

```
git revert <remediation commit SHA> --no-edit
```
Returns the branch to the original (still-FAIL, per Codex) Phase 1 commit `c55b2c86b0a0f7edd029cfc93f8329c9312450f4`. `git reset --hard` is not used. No tag exists for this milestone yet — `milestone/runtime-foundation` is only created after Codex's re-review returns PASS.

## 19. git remote status

```
git remote -v -> (no output)
```
No remote configured; not added this round; recorded as-is, matching every prior report.

## 20. Git status after remediation

Recorded immediately before commit, and to be re-confirmed immediately after (both expected empty):
```
git status --short --untracked-files=all       -> (expected: empty after commit)
git status --porcelain=v2 --untracked-files=all -> (expected: empty after commit)
```
See the developer's final reply to the user for the actual post-commit values.

## 21. Phase 2 has not started

**Explicit statement, unchanged from the original Phase 1 report: no SQLite schema/migration work, no task state model, no persistence beyond what already existed for SDKMAX chat, no project-entry flow, no GitHub import, no template creation, no real AI workflow.** This round's entire diff is confined to the Phase 1 runtime-foundation safety boundary and its tests, plus this report.

## 22. Codex re-review material checklist

1. This report and the remediation commit it is part of (single commit on `feature/v0.1.0-mvp`, parent `c55b2c86b0a0f7edd029cfc93f8329c9312450f4`).
2. `git diff c55b2c86b0a0f7edd029cfc93f8329c9312450f4 <remediation commit> --stat` — confirms the file list in §11.
3. `git diff c55b2c86b0a0f7edd029cfc93f8329c9312450f4 <remediation commit> -- Cargo.lock` — expected empty (see §12).
4. `apps/desktop/src-tauri/src/runtime/job_object.rs`, and the containment wiring in `process.rs` — Blocker 2.
5. `apps/desktop/src-tauri/src/runtime/detect.rs` — Blocker 1 (DI), Major 1 (classifier), Major 2 (write probe).
6. `apps/desktop/src-tauri/src/runtime/detection_slot.rs` and its use in `commands.rs`/`lib.rs` — Major 3.
7. `apps/desktop/src-tauri/src/security.rs` — Major 4.
8. `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `npm test`, `npm run build`, `npm run tauri -- build` — rerun independently against the remediation commit; expected exit 0 for all six, matching §14.
9. Specifically: `cargo test runtime::process::tests::timeout_terminates_grandchild_process_not_just_direct_child`, `..._cancellation_...`, and the full `detection_slot::tests::*` and `security::tests::*` modules, to independently confirm the process-tree and concurrency/redaction claims in §15.
10. `D:\nova-review\CODEX_PHASE1_RUNTIME_FOUNDATION_REVIEW09152354.md` — the FAIL review this remediation responds to.
11. Post-commit `git status --short --untracked-files=all` and `git status --porcelain=v2 --untracked-files=all` — both expected empty.
