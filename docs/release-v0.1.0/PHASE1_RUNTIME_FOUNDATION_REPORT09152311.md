# Nova v0.1.0-beta.1 Phase 1 — Runtime Foundation, Environment Detection, and Safety Boundary

Developer: Claude Code (main developer this phase)
Independent reviewer: Codex (not yet run for Phase 1)
Final acceptance: user
Generated: 2026-09-15 23:11 +0800 (Beijing time; local system clock verified UTC+8 in earlier sessions this week)
Repository: `D:\nova`

**This report documents Phase 1 development work. It does not claim Phase 1 is approved. Phase 1 is only complete after Codex's independent review returns PASS and the user accepts the result.**

---

## 1. Codex Stage 0 PASS report and reviewed HEAD

- Report: `D:\nova-review\CODEX_STAGE0_FINAL_PASS_REVIEW09152234.md`
- Final decision recorded in that report: **PASS**
- Exact HEAD that report reviewed: `c074c28d759c057ae6b8cc9ad4bc2ddb60d73336`
- That report's own text (section 1, "Reviewed Head") states the reviewed branch was `feature/nova-technical-mvp` at that exact commit, with both `git status --short --untracked-files=all` and `git status --porcelain=v2 --untracked-files=all` returning no output at review time.

## 2. Phase 1 branch and starting point

- Branch created this phase: `feature/v0.1.0-mvp`
- `git branch --list feature/v0.1.0-mvp` returned no output before creation (branch did not exist).
- Created with `git checkout -b feature/v0.1.0-mvp c074c28d759c057ae6b8cc9ad4bc2ddb60d73336` — an exact, non-guessed checkout of the Codex-reviewed commit.
- Post-creation verification:
  ```
  git rev-parse --abbrev-ref HEAD -> feature/v0.1.0-mvp
  git rev-parse HEAD              -> c074c28d759c057ae6b8cc9ad4bc2ddb60d73336
  git merge-base feature/v0.1.0-mvp c074c28d759c057ae6b8cc9ad4bc2ddb60d73336 -> c074c28d759c057ae6b8cc9ad4bc2ddb60d73336
  ```
  HEAD and merge-base both equal the Codex-reviewed SHA exactly — the branch starts from that commit with no divergence.

## 3. Git status before development

Captured on `feature/nova-technical-mvp` immediately before creating `feature/v0.1.0-mvp`:

```
git rev-parse --abbrev-ref HEAD -> feature/nova-technical-mvp
git rev-parse HEAD              -> c074c28d759c057ae6b8cc9ad4bc2ddb60d73336
git status --short --untracked-files=all       -> (no output)
git status --porcelain=v2 --untracked-files=all -> (no output)
git remote -v                                   -> (no output; no remote configured)
```

Last 10 commits at that point (unchanged from the Codex PASS report):
```
c074c28 docs(release): record pre phase 1 cleanup
b075a40 docs(release): record pre phase 1 cleanup
883850c docs(design): accept v0.1.0 uiux baseline
802efa7 docs(release): accept final stage 0 evidence
d75c28b docs(release): close stage 0 gate
e38269e docs(release): timestamp generated markdown files
da9d7f4 docs(release): close remaining stage 0 review gaps
823bbd9 docs(release): close stage 0 rereview gaps
5d5014e docs(release): record stage 0 baseline evidence
55e3cb9 chore: accept stage 0 remediation baseline
```

HEAD matched the Codex-reviewed SHA exactly and the working tree was clean — the required precondition for starting development was satisfied, no stop condition was triggered.

Root `AGENTS.md`, `CLAUDE.md`, and `README.md`/`README` are absent from the repository (confirmed via `Glob **/AGENTS.md`, `**/CLAUDE.md`, `**/README*` restricted to `D:\nova`, excluding `node_modules`). This matches `00_SCOPE09091050.md`'s own statement that no such files were found, and matches both prior Codex reviews' findings — recorded here, not treated as a new blocker.

## 4. Baseline test results (before Phase 1 changes)

No application source file had changed between the last full verified baseline and this phase's starting commit, so the recorded baseline in `D:\nova\test-results\build-info.txt` still applies without needing to be rerun:

```
Baseline commit: e1e967548621b78c2b9a78e2e08b43159c3040be
Baseline branch: feature/nova-technical-mvp
npm test          [PASS] exit=0
npm run build     [PASS] exit=0
cargo test        [PASS] exit=0 (23 Rust tests passed)
npm run tauri -- build [PASS] exit=0
Overall: PASS
```

This is confirmed still valid for the Phase 1 starting commit `c074c28` because every commit between `e1e9675` and `c074c28` (`d75c28b`, `802efa7`, `883850c`, `b075a40`, `c074c28` and their Stage 0 predecessors) was independently verified in the Stage 0 / Stage 0.5 / cleanup reports to touch only `docs/`, `product/`, `design/`, and the root market-research file — never `apps/`, `package.json`, lockfiles, or `apps/desktop/src-tauri/**`. No baseline failure exists to carry forward, and no Phase 1 result below can be an artifact of a pre-existing broken baseline.

## 5. Architecture and safety boundary

Before writing code, the existing Rust/Tauri implementation was read in full: `commands.rs`, `models.rs`, `security.rs`, `validation.rs`, `lib.rs`, `error.rs`, and the frontend `types.ts`, `security.ts`, `stores/nova.ts`, `App.vue`, plus `nova.test.ts` for existing test conventions. This is a Tauri 2 + Vue 3 + Rust 2021 + SQLite app; `tokio` (with `macros`, `rt-multi-thread`, `time` features) and `tokio_util::sync::CancellationToken` were already dependencies, already used for the existing chat-stream cancellation slot (`AppState.active_cancel`). **No dependency was added or upgraded and no `Cargo.toml`/`package.json`/lockfile changed** — the safety boundary below is built entirely on already-present dependencies (`std::process`, `std::thread`, `tokio::task::spawn_blocking`, `tokio_util::sync::CancellationToken`), reusing the same cancellation pattern the codebase already established for chat streaming rather than inventing a second one.

New module: `apps/desktop/src-tauri/src/runtime/` (`mod.rs`, `types.rs`, `path_guard.rs`, `process.rs`, `detect.rs`). One safe execution path — every future Git/Codex/Claude invocation is expected to go through `runtime::process::execute`, not a bespoke `Command::new` call in a Tauri command handler.

Boundary guarantees implemented in `runtime::process::execute_blocking`:

1. **Argument-array execution only.** `std::process::Command::new(&request.program).args(&request.args)` — arguments are always a `Vec<String>`, never concatenated into a shell string. No `cmd.exe`, `powershell`, or `sh -c` wrapper is used to run a target tool.
2. **cwd boundary check before spawn.** `request.cwd` is resolved through `path_guard::validate_directory_boundary` against `request.allowed_root` *before* `Command::spawn` is ever called. A request with `allowed_root: None` is rejected outright (`PathRejected`) rather than defaulting to "no boundary."
3. **stdout/stderr captured separately** via two OS threads reading each pipe independently (`spawn_capped_reader`), so a large single stream cannot deadlock the other or the child.
4. **Exit code, start time, end time, duration, and a classified result** (`ExecutionOutcome::Success` or `Failed { kind, detail }`) are recorded for every execution, including ones that never got as far as spawning (path rejection, spawn failure).
5. **Timeout** (`ExecutionRequest.timeout`, default 20s, 8s for CLI detection) is enforced by a poll loop checking `child.try_wait()` against elapsed time.
6. **Cancellation** via `tokio_util::sync::CancellationToken`, checked in the same poll loop — the same primitive already used for chat cancellation, now generalized.
7. **On timeout or cancellation the child is killed and then `wait()`ed on** (never left running, never left as a zombie) before the function returns. Verified by two dedicated tests (`timeout_kills_the_process_and_reports_timed_out`, `user_cancellation_kills_the_process_and_reports_cancelled`) that spawn `ping` *directly* (not via `cmd /C`) specifically so the killed process has no un-killed grandchild.
8. **Output length is capped** (`ExecutionRequest.max_output_bytes`, default 64 KiB) — the reader threads keep draining the pipe past the cap (so the child is never blocked on a full pipe) but discard bytes beyond the cap and set a `_truncated` flag.
9. **All captured stdout/stderr is redacted** via `security::redact_sensitive_text` before being stored in `ExecutionResult` — raw output is never held anywhere after the reader thread returns.
10. **Non-success outcomes are never reported as `Success`.** A non-zero exit is `Failed { kind: NonZeroExit, .. }`; a timeout is `Failed { kind: TimedOut, .. }`; a cancellation is `Failed { kind: Cancelled, .. }` — there is no code path that turns any of these into `Success`.
11. Windows child processes are spawned with `CREATE_NO_WINDOW` so detection commands never flash a console window.

`runtime::path_guard::validate_directory_boundary` defeats `..` traversal and symlink/junction escapes by canonicalizing **both** the candidate and the allowed root with `std::fs::canonicalize` (which resolves `..` and reparse points to their real target) *before* comparing with `Path::starts_with` — a path that looks like it's inside the root as a string but actually resolves outside it via a junction is rejected because the canonical target fails the prefix check, not the pre-resolution string.

`runtime::detect` builds on `process::execute` for CLI detection (`git`/`codex`/`claude` with `--version`, run from Nova's own app data directory, never a user project directory) and adds `check_directory_access` (canonicalize + is_dir + a create-then-remove write probe) for both the app data directory and — ready for Phase 3+ — a user-selected project directory, via the exposed `check_project_directory_access` command.

## 6. Modified/new files

New (`apps/desktop/src-tauri/src/runtime/`):
- `mod.rs`
- `types.rs` — `ToolKind`, `ToolStatus`, `ExecutionErrorKind`, `ExecutionOutcome`, `ExecutionRequest`, `ExecutionResult`
- `path_guard.rs` — directory boundary/traversal validation
- `process.rs` — the safe process executor
- `detect.rs` — CLI/platform/directory detection built on `process.rs`

Modified:
- `apps/desktop/src-tauri/src/lib.rs` — registers the `runtime` module; adds `app_data_dir` and `detection_cancel` to `AppState`; registers the three new Tauri commands.
- `apps/desktop/src-tauri/src/commands.rs` — adds `detect_runtime_environment`, `cancel_environment_detection`, `check_project_directory_access`.
- `apps/desktop/src-tauri/src/security.rs` — extends `redact_sensitive_text` (see §8).
- `apps/desktop/src/App.vue` — adds a fourth, minimal "环境" (Environment) tab.
- `apps/desktop/src/types.ts` — adds `ToolStatus`, `DirectoryStatus`, `PlatformStatus`, `RuntimeEnvironmentReport`, mirroring the Rust serde shapes exactly.
- `apps/desktop/src/stores/nova.ts` — adds `environment`/`environmentLoading`/`environmentError` state and `detectEnvironment`/`cancelEnvironmentDetection` actions.
- `apps/desktop/src/stores/nova.test.ts` — adds a new `describe` block for the store actions above.

**Not touched:** `apps/desktop/src/App.vue`'s existing chat/providers/settings tabs (only additive changes), `package.json`, any lockfile, `Cargo.toml`, `tauri.conf.json`, `.github/`, `scripts/`, product/scope docs, `design/uiux-v0.1.0/`.

## 7. CLI/tool status model

`runtime::types::ToolStatus` (serialized with `#[serde(tag = "state")]`) defines: `NotFound`, `Ready { version }`, `NotLoggedIn`, `SessionExpired`, `QuotaExceeded`, `PermissionDenied`, `NetworkInterrupted`, `StartFailed { detail }`, `NonZeroExit { exit_code, detail }`, `TimedOut`, `Cancelled`, `Unknown { detail }` — all 12 states the task specified, satisfying "must not judge readiness from `--version` alone."

**What is real, tested evidence in this phase, and what is architecturally defined but not yet triggerable from real observed CLI output:**

- `NotFound`, `Ready`, `StartFailed`, `NonZeroExit`, `TimedOut`, `Cancelled`, `PermissionDenied`, `Unknown` are all reached from a real, classified process outcome (spawn error kind, exit code, timeout, or cancellation) and are covered by tests using the actually-installed `git` binary and deliberately-missing/failing commands. This machine's real state was probed before writing any detector: `git --version` → `git version 2.55.0.windows.2` (exit 0); `Get-Command claude`/`Get-Command codex` on native Windows PATH → both **not found** (the running "Codex" process on this machine is the packaged desktop app `D:\软件下载\...\Codex.exe`, not a `codex` CLI on PATH). `detect_tool(ToolKind::CodexCli, ...)` and `detect_tool(ToolKind::ClaudeCli, ...)` therefore return real `NotFound` results on this machine, and are tested as such — not simulated.
- `NotLoggedIn`, `SessionExpired`, `QuotaExceeded`, `NetworkInterrupted` are defined in the type (so the state model and its JSON shape are complete and the frontend can render them) but **no code path currently produces them**, because neither the Codex CLI nor the Claude Code CLI is installed in this environment, so there is no real, observed output format to classify against — and the task instructions explicitly forbid inventing CLI behavior without evidence ("不得凭空编造"). A `codex`/`claude` non-zero exit today is reported as `NonZeroExit` with the tool's own (redacted) stderr text, not guessed as one of these four. Wiring exact detection for these four states is listed under Known Limitations (§16) as follow-up work once real CLI output evidence is available.

## 8. Redaction rules

`security::redact_sensitive_text` (already the shared entry point used by the SDKMAX chat error path) was extended, not replaced, and is now also the single redaction call every `runtime::process` execution result passes through. Three passes, each preserving whitespace/newlines exactly (multi-line CLI output must stay readable, unlike the previous `split_whitespace().join(" ")` implementation which would have collapsed it):

1. Header/scheme markers: `Authorization: Bearer `, `Cookie: ` (existing), plus newly added bare `Bearer ` (so a token following "Bearer" without an "Authorization:" prefix is still caught).
2. Generic `key=value` / `key: value` scan (character-by-character, not regex — no new dependency) for key names containing `password`, `passwd`, `secret`, `token`, `key`, or `credential` — covers API keys, `access_token`/`refresh_token`, environment-variable-style secrets, and `?token=`/`&api_key=`-style URL query parameters, without swallowing the next query parameter.
3. Bare prefixed tokens with no `key=` shape at all: existing OpenAI/Anthropic-style `sk-...`, plus newly added GitHub `ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_`/`github_pat_`.

9 new tests in `security.rs` cover: bare Bearer, Cookie header, env-style API key + password, access/refresh tokens, URL query token+key (without eating the next parameter), GitHub token prefix, newline/multiline preservation, and a negative test confirming ordinary non-sensitive `key=value` pairs are left untouched. All test secrets are fabricated placeholders (e.g. `["sk", "secret-secret"].join("-")`), never real credentials.

## 9. Path and command-injection protection

- Boundary: see §5 / `path_guard.rs`. Tests cover the root itself, a plain subdirectory, a Chinese-and-space-containing subdirectory, `..` traversal that escapes the root (rejected), `..` that resolves back inside the root (allowed), a nonexistent path, a file used as a directory, a same-string-prefix sibling directory (`root-evil` vs `root`, rejected), an unreachable UNC-style path (rejected safely, no panic), a relative `..` path from the current directory (rejected), and — running for real in this environment — a Windows directory junction created with `mklink /J` that escapes the root (rejected; test also degrades gracefully if `mklink` is unavailable, but it succeeded here).
- Injection: `process.rs::argument_containing_shell_metacharacters_is_not_reinterpreted` passes the literal string `phase1 && calc.exe; rm -rf / | echo pwned > out.txt` as a single element of the args array to the real `git` binary, and asserts (a) `git` exits non-zero and (b) the literal string appears verbatim in `git`'s own stderr — proof the whole string arrived as one argument, not split by a shell, because no shell was ever invoked to run `git`.

## 10. New tests (Rust)

All under `apps/desktop/src-tauri/src/runtime/` and `security.rs`, using `std::env::temp_dir()` fixtures (never a real user project) and the actually-installed `git`/`ping`/`cmd` system binaries or deliberately-missing program names — no mocks that could hide a real integration bug:

`path_guard.rs` (11 tests): root itself; plain subdirectory; Chinese+space path; `..` traversal rejected; `..` resolving back inside allowed; nonexistent path; file-as-directory; same-prefix sibling rejected; unreachable UNC path; relative `..` escape; directory junction escape (real `mklink /J`).

`process.rs` (9 tests): reject execution outside allowed root (no spawn); missing executable → NotFound; `git --version` success with stdout capture; non-zero exit with separated stderr; timeout kills the process; user cancellation kills the process; output over cap is truncated; shell-metacharacter argument not reinterpreted.

`detect.rs` (11 tests): platform detection; directory access ready/not-found/not-a-directory; classification of Ready/NotFound/StartFailed/TimedOut/Cancelled/PermissionDenied/NonZeroExit; real `git` detected as Ready; real `codex`/`claude` detected as NotFound (this machine's actual PATH state); full report aggregation.

`security.rs` (+9 tests, on top of the 2 pre-existing): see §8.

## 11. Local verification commands, exact results

All run from `D:\nova\apps\desktop\src-tauri` (Rust) or `D:\nova\apps\desktop` (frontend) unless noted. All commands and exit codes below are from this session's actual run, 2026-09-15 23:04–23:11 +0800.

| # | Command | Working dir | Start | Exit | Result |
|---|---|---|---|---|---|
| 1 | `cargo fmt --check` | `apps/desktop/src-tauri` | 23:04:07 | non-zero (formatting diffs only, see below) | Diffs shown, no logic issues |
| 2 | `cargo fmt` (apply) + `cargo fmt --check` | `apps/desktop/src-tauri` | 23:04 | 0 | PASS — clean |
| 3 | `cargo clippy --all-targets --all-features -- -D warnings` (1st run) | `apps/desktop/src-tauri` | 23:04:25 | 101 | 3 `dead_code` errors (unused convenience methods) + `ptr_arg`/`useless_vec` — all fixed (see below) |
| 4 | `cargo clippy --all-targets --all-features -- -D warnings` (2nd run) | `apps/desktop/src-tauri` | after fixes | 0 | PASS — 0 warnings |
| 5 | `cargo test` | `apps/desktop/src-tauri` | 23:08:28 | 0 | PASS — **61 passed, 0 failed** (baseline 23; net +38 new, 0 regressions) |
| 6 | `npm test` | `apps/desktop` | 23:09:20 | 0 | PASS — **13 passed, 0 failed**, 3 files (baseline 10; net +3 new, 0 regressions) |
| 7 | `npm run build` (`vue-tsc --noEmit && vite build`) | `apps/desktop` | 23:09:36 | 0 | PASS — type check clean, production bundle built (`dist/`, 85.87 kB JS, 2.81 kB CSS) |
| 8 | `npm run tauri -- build` | `apps/desktop` | 23:09:57 | 0 | PASS — release Rust compile finished in 1m 11s, `target\release\nova.exe` built. `bundle.active: false` in `tauri.conf.json` (pre-existing, unchanged), so no NSIS installer is produced by design — this matches the existing project configuration, not a failure. |

Step 3 detail (clippy failures found and fixed, not silently worked around): three helper methods (`ToolKind::label`, `ToolStatus::is_ready`, `ExecutionResult::is_success`) were only referenced from `#[cfg(test)]` code, which Rust's `dead_code` lint flags when compiling the plain `lib` target. Fixed by removing the three convenience methods and inlining the equivalent `matches!(...)` check at each of their 6 test call sites — no behavior change, just less unused production surface. Two more clippy style findings (`&PathBuf` parameter that should be `&Path`; a `vec![...]` immediately followed by `.iter().collect()` that should just be a plain `Vec` literal) were fixed the same way. Full before/after logs were kept during the session; final state is `cargo fmt --check` and `cargo clippy -D warnings` both clean.

No environment-limited or skipped step. No baseline failure existed to carry forward (§4).

## 12. Items not passed, and environment limitations

None outstanding. Every command above ended exit 0 in its final run. The one environment-dependent test (`path_guard::rejects_directory_junction_that_escapes_root`, which needs `mklink /J`) is designed to skip gracefully rather than fail if unavailable, but it ran for real and passed on this machine.

## 13. Exact Phase 1 feature commit SHA

**Deliberately not written in this file.** This report is committed in the same single commit as the code it describes (per this phase's Git-boundary instructions), so the commit's own SHA cannot be known at the time this file's content is finalized — writing a guessed or pre-computed SHA here would violate the "must not fabricate or predict a SHA" rule. The authoritative SHA is: (a) the output of `git log -1 --format=%H` on branch `feature/v0.1.0-mvp` immediately after this commit, and (b) reported verbatim in the developer's final reply to the user for this task. Do not trust any SHA that might appear to have been embedded in an earlier draft of this file; this file's committed version deliberately omits it.

## 14. git remote status

```
git remote -v -> (no output)
```
No remote is configured. Per this phase's instructions this does not block local Phase 1 development, but it remains a registered blocker for any future GitHub-based CI, PR, or release-artifact work (already flagged identically by the Codex Stage 0 PASS review, §14 there).

## 15. Rollback method

This phase produced exactly one feature commit on `feature/v0.1.0-mvp`. To roll back:
```
git revert <Phase 1 feature commit SHA> --no-edit
```
`git reset --hard` is not used. The commit's parent is `c074c28d759c057ae6b8cc9ad4bc2ddb60d73336` (the Codex-reviewed Stage 0 baseline), so a revert returns the branch to that exact, previously-approved state. No tag exists yet for this milestone — per this phase's instructions, `milestone/runtime-foundation` is only to be tagged after Codex's Phase 1 review returns PASS, not before.

## 16. Known limitations

- `NotLoggedIn`, `SessionExpired`, `QuotaExceeded`, `NetworkInterrupted` are defined in `ToolStatus` but not yet wired to real detection logic for the Codex/Claude CLIs, because neither is installed in this development environment and no real output evidence exists to classify against without guessing (§7). A non-zero exit from either CLI today is honestly reported as `NonZeroExit` with the tool's own redacted stderr, not misclassified as one of these four.
- `PlatformStatus` reports only `std::env::consts::OS`/`ARCH` and a `cfg!(windows)`-derived `is_supported` flag; it does not probe a specific Windows build/version number. Deeper OS version detection (if ever needed) is out of Phase 1 scope and was not added to avoid a new system-info dependency.
- `check_project_directory_access` is exposed and tested now (reusing the same `check_directory_access` logic as the app data directory check) but has no caller yet in the UI — project directory selection is Phase 3/5/6 scope. It is wired early only so the boundary logic itself is exercised and tested end to end in Phase 1, per the phase's own "Chinese-path/space-path/unauthorized-directory tests" requirement.
- The Phase 1 "环境" (Environment) tab is intentionally minimal (platform, app data directory, three CLI statuses, a re-detect/cancel button, and a collapsed redacted-diagnostics panel) and does not attempt any Phase 3+ UI (project entry, GitHub import, template creation, task timeline, or real AI workflow).
- `npm run tauri -- build` does not produce an NSIS installer because `tauri.conf.json`'s `bundle.active` is `false` — this is pre-existing project configuration from before Phase 1 and was not changed.

## 17. Phase 2 has not started

**Explicit statement: Phase 2 (Task State Machine, Local Storage, and Interruption Recovery) has not been started.** No SQLite schema/migration work, no task state model, and no persistence beyond what already existed for SDKMAX chat was added. Nothing in `apps/desktop/src-tauri/src/db.rs` was touched.

## 18. Codex review material checklist

1. This report and the Phase 1 feature commit it is part of (single commit on `feature/v0.1.0-mvp`, parent `c074c28d759c057ae6b8cc9ad4bc2ddb60d73336`).
2. `git show <commit>` / `git diff c074c28d759c057ae6b8cc9ad4bc2ddb60d73336 <commit> --stat` — confirms the file list in §6 and that no path outside it changed.
3. `git diff c074c28d759c057ae6b8cc9ad4bc2ddb60d73336 <commit> -- package.json package-lock.json apps/desktop/package.json apps/desktop/package-lock.json apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/Cargo.lock apps/desktop/src-tauri/tauri.conf.json .github scripts .gitignore` — expected empty, confirming no dependency/config/lockfile/CI change.
4. `apps/desktop/src-tauri/src/runtime/{mod,types,path_guard,process,detect}.rs` — the new safety boundary and detection code, plus their inline tests.
5. `apps/desktop/src-tauri/src/security.rs` — the extended redaction and its 9 new tests.
6. `apps/desktop/src/{App.vue,types.ts,stores/nova.ts,stores/nova.test.ts}` — the minimal Environment tab.
7. `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `npm test`, `npm run build`, `npm run tauri -- build` — rerun independently against the Phase 1 commit; expected exit 0 for all six, matching §11.
8. `D:\nova-review\CODEX_STAGE0_FINAL_PASS_REVIEW09152234.md` — the Stage 0 PASS this phase built on.
9. `docs/release-v0.1.0/03_IMPLEMENTATION_MAP09091050.md` Phase 1 section and `02_GAP_MATRIX09091050.md` "Required CLI Status Model" — the authoritative scope this report claims to satisfy.
10. Post-commit `git status --short --untracked-files=all` and `git status --porcelain=v2 --untracked-files=all` — both expected empty.
