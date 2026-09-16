# Phase 3 Remediation Report — Open Local Project Dual-AI Core Loop

Generated: 2026-09-16 21:55 (Beijing time)

## 1. Scope and starting point

- Pre-fix commit (original Phase 3 implementation): `20a1f282537144451b83b05d2a1dc1457011d0aa`
- Branch: `feature/v0.1.0-mvp`
- Codex review being remediated: `D:\nova-review\CODEX_PHASE3_OPEN_LOCAL_PROJECT_CORE_LOOP_REVIEW09161737.md` — verdict **FAIL**, 4 Blockers / 5 Majors / 3 Minors / 3 Suggestions.
- Mandate: close all 4 Blockers and all 5 Majors, close Minors where feasible, transform Phase 3 from a mock-verified backend skeleton into a user-operable, real-CLI-runnable, security-boundary-sound minimal closed loop. Do not enter Phase 4. Do not create `milestone/core-loop`. Deliver for Codex's independent re-review.

## 2. Pre-check results

Verified before any work began (per Section 一 of the task):
- Branch: `feature/v0.1.0-mvp` ✓
- HEAD: `20a1f282537144451b83b05d2a1dc1457011d0aa` ✓ (matches expected)
- Worktree: clean at start ✓
- `milestone/runtime-foundation` → `f91dd62ad12f934ad40ccff87c2a966df24fc620` ✓ (unchanged)
- `milestone/task-engine` → `b6e542c448b71e6e1ac79f865e2548c1de9a311f` ✓ (unchanged)
- `milestone/core-loop` did not exist at start, and does **not** exist now ✓

## 3. Post-fix commit

This report is written immediately before the single remediation commit is created. The commit SHA is reported in the final summary message (outside this document, per the "final reply must contain only" list), and this report will be included in that same commit.

## 4. Complete changed-file list

New files:
- `apps/desktop/src-tauri/src/app_paths.rs` — Blocker 4 path-safety primitives
- `apps/desktop/src-tauri/src/core_loop/review_isolation.rs` — Blocker 1 isolated reviewer workspace
- `apps/desktop/src-tauri/src/core_loop/test_runner.rs` — Blocker 3 independent test execution
- `apps/desktop/src-tauri/src/core_loop/documents.rs` — Major 5 seven-document generation
- `apps/desktop/src-tauri/src/runtime/verdict.rs` — Major 2 structured JSON verdict protocol
- `apps/desktop/src/stores/coreLoop.ts` — Major 1 frontend store
- `apps/desktop/src/stores/coreLoop.test.ts` — Major 1 frontend tests
- `apps/desktop/src/components/CoreLoopView.vue` — Major 1 minimal UI

Modified files:
- `apps/desktop/src-tauri/src/commands.rs` — readiness/test-command/cancel commands, cross-project-safe review listing
- `apps/desktop/src-tauri/src/core_loop.rs` — Blocker 1/3/4, Major 2/4/5 orchestration wiring
- `apps/desktop/src-tauri/src/lib.rs` — new command registration, per-task cancellation registry, app-shutdown cancellation hook
- `apps/desktop/src-tauri/src/models.rs` — `Task.test_command_*`, richer `TestEvidence`
- `apps/desktop/src-tauri/src/runtime/ai_adapter.rs` — Blocker 2: `CodexCliAdapter`/`ClaudeCliAdapter`/`DualCliAdapter`/`AdapterReadiness`
- `apps/desktop/src-tauri/src/runtime/detect.rs` — widened `infra_status` visibility (reused by Blocker 2 probes)
- `apps/desktop/src-tauri/src/runtime/mod.rs` — registered `verdict` module
- `apps/desktop/src-tauri/src/runtime/process.rs` — stdin-piping support (Blocker 2)
- `apps/desktop/src-tauri/src/runtime/types.rs` — `ExecutionRequest.stdin`
- `apps/desktop/src-tauri/src/runtime/win_spawn.rs` — stdin pipe plumbing for `CreateProcessW`
- `apps/desktop/src-tauri/src/storage/evidence.rs` — richer `record_test_evidence`, `list_test_evidence_for_round`
- `apps/desktop/src-tauri/src/storage/migrations.rs` — schema v5 (`task_reviews` ownership) + v6 (`test_command`, richer `test_evidence`)
- `apps/desktop/src-tauri/src/storage/reviews.rs` — Major 3 project-ownership checks
- `apps/desktop/src-tauri/src/storage/tasks.rs` — `test_command_redacted`/`test_command_source` columns, `set_task_test_command`
- `apps/desktop/src-tauri/tests/fixtures/nova_test_helper.rs` — `fake-cli` scriptable mode for Blocker 2 tests
- `apps/desktop/src/App.vue`, `apps/desktop/src/stores/nova.ts`, `apps/desktop/src/types.ts` — new tab wiring, Phase 3 types

## 5. Blocker 1 — Reviewer technical read-only enforcement

**Design**: `core_loop::review_isolation::prepare()` copies the project's working tree (excluding `.git`, never following a symlink/junction encountered in the tree — proven with a real `mklink /J`) into a disposable, app-owned directory (`<app-data>/review-workspaces/<project-id>/<task-id>/round-<n>/<uuid>/source/`). `runtime::ai_adapter::ReviewRequest` was restructured to have **no field carrying the real project path at all** — it only carries `isolated_workspace_dir`. This is a compile-time-enforced guarantee: no code path can hand the reviewer adapter the real project directory, because the type doesn't have one to hand.

The reviewer's `cwd`/argv reference only the isolated copy. Even a fully compromised/malicious reviewer process with unrestricted write access to its own `cwd` is physically unable to reach the original project — proven directly (`reviewer_is_never_given_the_real_project_path_at_all` test captures the actual `ReviewRequest.isolated_workspace_dir` passed to the adapter and asserts it is neither equal to nor nested inside the real project directory).

The old git-status/HEAD before/after comparison is **retained**, but now explicitly documented and used only as a **supplementary audit signal** on the real project — never the primary boundary.

**Adversarial tests** (`core_loop::review_isolation::tests`, all passing):
- writes into the isolated copy never reach the real project (file write, file delete)
- a real `git commit` executed inside the isolated copy never changes the real repo's HEAD or tracked content
- `.git` is excluded from the copy entirely
- a symlink/junction planted inside the project is skipped, never followed, during the copy (real `mklink /J`)
- the isolated copy path is provably outside and non-overlapping with the real project directory
- cleanup refuses to delete anything outside the expected `review-workspaces` root
- `task_id`/`project_id` path-traversal attempts (`../escape`) are rejected before any copy happens
- each round gets its own disposable workspace (crash-recovery-safe)

Not separately tested with a literal `.env` filename, but the general "any file write inside the copy never reaches the original" test covers the identical mechanism regardless of filename.

## 6. Blocker 2 — Real CLI adapters

**Investigation first**: `where.exe codex`, `where.exe claude`, `Get-Command codex`/`claude`, and direct `--version` invocation were all attempted in this execution environment. **Neither binary is installed or reachable** in this session's shell (both PATH lookups and direct invocation fail; no local install found under common locations; `npm list -g` shows neither package). This is recorded honestly rather than glossed over — see Section 15 below.

**Design**: `CodexCliAdapter` and `ClaudeCliAdapter` are genuinely separate structs with separate argv construction (`runtime::ai_adapter.rs`), composed behind `DualCliAdapter` which dispatches on `request.ai_kind` so a role swap always uses the correct adapter's own flags. `CliConfig::binary_path` is configurable (read from `app_settings.codex_binary_path`/`claude_binary_path` via the existing generic `get_setting`/`set_setting` commands, falling back to PATH-resolved bare names) — not PATH-only.

Task package content is sent to the CLI as real **stdin content**, never as a file path masquerading as a prompt — this required adding genuine stdin-piping support to Phase 1's Windows process boundary (`win_spawn.rs`: a real inheritable pipe for the child's stdin, written from a dedicated writer thread to avoid a stdout/stdin deadlock; `process.rs`: the same pattern on the non-Windows path). This is an **additive** change to the already-accepted Phase 1 module — no existing behavior changed for callers that don't opt in via `.with_stdin()`.

**Readiness probing** (`probe_readiness`): real `--version` + `--help` (+ a subcommand `--help` for Codex's `exec`) invocations, classified via the same `runtime::detect::classify_tool_readiness` Phase 1 already uses for login/session/quota state, plus keyword-based capability confirmation (`exec` for Codex dev capability, `sandbox`/`read-only` for Codex review capability; `-p`/`--print` for Claude dev capability, `permission-mode` for Claude review capability). Dev/review readiness is **fail-closed**: if the CLI is not found, not logged in, or the capability keywords are not confirmed in real `--help` output, `dev_ready()`/`review_ready()` return `false` and `commands::start_core_loop` refuses to transition the task or call any AI, returning a clear Chinese message naming exactly what's missing.

**Fake-executable integration tests** (`runtime::ai_adapter::tests`, all passing): `tests/fixtures/nova_test_helper.rs` gained a `fake-cli` mode, triggered by the **launched executable's own file identity** (a renamed/hard-linked copy whose stem contains `fake-cli`) rather than an environment variable — this was a deliberate fix after an initial env-var-based design caused a real cross-test race (see Section 20). Tests verify: real argv/stdin/cwd reach the fake CLI; Codex sends `--sandbox workspace-write`/`read-only`; Claude sends `-p --permission-mode acceptEdits`/`plan`; role swap uses the correct adapter's own flags; a non-zero exit with `"approve"`-shaped text embedded in stdout is still reported as failed, never success; Windows Unicode/space-containing paths work end to end; readiness correctly reports `NotFound` for a genuinely missing binary and correctly confirms/denies capability from real `--help` text.

**Honesty statement**: the exact real-world flag names (`--sandbox`, `--permission-mode`, etc.) are best-effort from public documentation, not confirmed against a live binary in this environment. The readiness probe is designed to fail closed rather than silently misfire if a real installed version's flags differ — see Section 15.

## 7. Blocker 3 — Independent test execution

**Design**: `core_loop::test_runner` provides read-only auto-detection (`Cargo.toml` → `cargo test`; `package.json` + lockfile → `npm`/`pnpm`/`yarn test`; `pyproject.toml`/`pytest.ini`/`setup.cfg` → `python -m pytest`) and a strict allowlist (`is_allowlisted`) that is the **only** gate for what Nova will ever execute as a "test run" — never an AI-suggested arbitrary command. `run_tests_for_round` in `core_loop.rs` is gated through the Action Ledger (`test-{task_id}-round-{round}`, its own key per round), runs the command through Phase 1's safe process boundary, and records independent `test_evidence` (schema v6: `result_generation`, `round`, `status` ∈ `{passed, failed, no_tests_applicable, timed_out, cancelled, insufficient_evidence}`, counts, redacted `stdout_ref`/`stderr_ref`).

A genuine test **failure** routes the round straight to `NeedsFix` — the reviewer is **never even invoked** — so a reviewer's `approve` can never bypass a real test failure. Verified end to end with a real, freshly-compiled failing Rust crate (`a_real_cargo_test_failure_is_independently_detected_and_blocks_preview_ready`): Nova runs real `cargo test` against a fixture crate with one failing `#[test]`, independently observes the failure, and the task ends in `Failed` after exhausting the one automatic fix pass, with `review_call_count() == 0`.

`finish_success_path` (the only path to `preview_ready`) re-verifies directly against the database that the current round has a `passed`/`no_tests_applicable` test-evidence row — defense in depth beyond the structural guarantee, so a future code change adding another path to `Approved` can never silently skip the gate (`finish_success_path_refuses_to_advance_without_current_round_test_evidence`).

**Known, honestly-reported gap**: this round wires Nova's own auto-detection and the allowlist/execution/evidence/gating machinery fully, plus a `detect_test_command`/`confirm_task_test_command` command pair the minimal UI uses. It does **not** implement a blocking, synchronous "must get an explicit user click before proceeding" interaction specifically for the `no_tests_applicable` case mid-loop — when no test framework is detected and no command has been confirmed, Nova records `no_tests_applicable` (Nova's own determination, never the AI's) and proceeds without blocking, since the confirmation UI (Section 6, Major 1) lets the user confirm/edit *before* starting, but the loop itself does not currently pause and wait for a fresh confirmation if the user skipped that step. This is a real, scoped limitation, not a silent gap — documented here per the report's own honesty requirement.

**New tests**: npm/pnpm/yarn/cargo/pytest detection, no-framework detection, allowlist accept/reject (including AI-suggested arbitrary commands: `bash -c "rm -rf /"`, `python -c "..."`), output-count parsing for cargo/pytest/Jest-style formats (plus a regression test for stdout+stderr concatenation shadowing — see Section 20), timeout classification, cancellation classification, a real end-to-end `cargo test` run against this very crate, a non-allowlisted user-configured command refused before execution, round-2-reruns-independently, and the real-failing-crate integration test above.

## 8. Blocker 4 — App-owned task package path safety

**Design**: `app_paths.rs` provides `sanitize_id_component` (rejects `..`, separators, drive/UNC prefixes, reserved Windows device names, control characters, trailing dot/space), `safe_app_owned_dir` (ancestor-by-ancestor symlink/junction rejection via `symlink_metadata`, final-path containment verification via `canonicalize` + `starts_with`, and a **race-safe** create-or-verify path so concurrent callers building the same directory never spuriously fail), `atomic_write` (temp-file-then-rename), and `safe_remove_dir_all` (root-verified, never-follows-symlinks-inside-target deletion).

Task packages moved from `<project>/.nova/tasks/<task-id>/` to `<app-data>/task-packages/<project-id>/<task-id>/round-<n>/` — proven never to touch the project directory at all (`task_packages_are_written_outside_the_project_directory_entirely`: asserts `.nova` is never created under the project root).

**Adversarial tests**: `task_id`/`project_id` containing `../escape`, `..\escape`, a path separator, an absolute/drive-prefixed value (`C:\Windows\System32`), a UNC path (`\\server\share`), reserved device names (`CON`, `NUL`, `con`, `PRN`, `COM1`), trailing dot/space — all rejected. A real junction (`mklink /J`) planted as an ancestor of `task-packages` is rejected, not followed. Concurrent calls building the identical nested directory never spuriously fail (the real bug found and fixed during this round — see Section 20). Different rounds get different directories.

## 9. Major 1 — Minimal frontend UI

New `apps/desktop/src/components/CoreLoopView.vue` + `apps/desktop/src/stores/coreLoop.ts`, wired into `App.vue` as a new "项目任务" tab. Every action calls a real Tauri command — nothing is a static demo: open/select project (`create_project`/`list_projects`), create task (`create_task`), confirm requirement (`transition_task` draft→awaiting_confirmation→confirmed), detect/confirm test command (`detect_test_command`/`confirm_task_test_command`), check CLI readiness (`get_core_loop_readiness`), start (`start_core_loop`, disabled unless both dev and review readiness are confirmed, with a `starting` reentrancy guard against duplicate clicks), cancel (`cancel_core_loop`), poll state (`list_tasks_for_project`), view review verdicts (`list_task_reviews`), accept/reject (`record_user_acceptance` + `transition_task`).

Role selection (`devAiKind`) is locked (`canChangeRoles` getter) once the task has left `draft`/`awaiting_confirmation`. Explicitly **not** implemented, matching the mandated exclusions: the full Phase 4 timeline, the evidence center, GitHub import, Create Product, auto-deployment, push/PR.

**New frontend tests** (`stores/coreLoop.test.ts`, 6 tests, all passing): task creation via a real mocked backend call; a backend error surfaces into `lastError` rather than being swallowed; `start_core_loop` is never called merely by checking readiness when not ready; the `starting` reentrancy guard prevents a second concurrent `start_core_loop` call while one is in flight; role selection locks once the task leaves the pre-start stage; role-swap pairing (`reviewerAiKind` always the opposite of `devAiKind`).

**Verification performed**: `vue-tsc --noEmit` (zero errors), `vite build` (succeeds), `vitest run` (19/19 passing, including the 6 new tests). **Honest limitation**: a live, interactive browser/Tauri-window click-through was not performed in this session — this is a Tauri desktop app requiring the native IPC bridge for real backend calls, and the available browser-preview tooling in this execution environment is scoped to a different working directory than the Nova project, which the permission system correctly flagged as out of scope when an initial config write was attempted outside the project. TypeScript compilation, production build, and comprehensive mocked-backend unit tests are the verification actually performed and are reported as such, not overstated as a full manual UI walkthrough.

## 10. Major 2 — Structured JSON verdict protocol

New `runtime::verdict` module. Replaces free-text `NOVA_REVIEW_VERDICT: ...` line-scanning entirely with strict parsing of exactly one JSON object (`{"schema_version":1,"task_id":...,"result_generation":...,"round":...,"verdict":...,"summary":...,"findings":[...]}`), extracted either from a single fenced ` ```json ` code block (if exactly one exists) or, if none, the *entire* trimmed output (which must then be nothing but the object). Duplicate top-level JSON keys are explicitly rejected via a custom `Visitor` (not merely "last value wins", `serde_json`'s default). `task_id`/`result_generation`/`round` must match the current task context exactly. Any parse failure — missing/duplicate/extra fields, wrong context values, multiple JSON candidates, truncated JSON, unknown verdict — uniformly routes to `insufficient_evidence`, never a guess or a silent default-approve.

`core_loop.rs`'s review round was rewired to use this protocol; the review package now instructs the reviewer with the exact schema and the task's real `task_id`/`result_generation`/`round`.

**17 tests**, all passing: the three normal verdicts; parsing from a fenced block with surrounding prose; rejecting multiple fenced blocks (proves a markdown example alongside the real answer is never silently picked); rejecting duplicate `verdict` fields; missing required fields; wrong `task_id`/`result_generation`/`round`; truncated JSON; project-file content containing the word "approve" never mistaken for a verdict; unknown verdict value; unsupported schema version; extra text around an unfenced object rejected; a bare whitespace-padded object accepted; findings round-trip; unknown extra fields tolerated.

## 11. Major 3 + schema v5 — `task_reviews` project ownership

New migration `V5_SQL` (append-only; `V4_SQL` untouched). Adds a `UNIQUE INDEX idx_tasks_id_project_id ON tasks(id, project_id)` and rebuilds `task_reviews` with `FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id)` plus `CHECK(round IN (1,2))`. The migration's `INSERT ... SELECT ... INNER JOIN tasks ... WHERE tr.round IN (1,2)` copies forward only genuinely consistent rows; any V4-era row whose `project_id` disagreed with its task's real project, or whose `round` was outside `{1,2}`, does not survive the upgrade (proven by `v4_to_v5_upgrade_drops_a_review_row_whose_project_id_does_not_match_its_task`).

`storage/reviews.rs`: both `record_task_review` and `list_task_reviews` now re-verify ownership in Rust (reusing `storage::evidence`'s `assert_task_belongs_to_project` guard) before the FK is even reached, giving a clean error instead of a raw FK-violation. `list_task_reviews` now **requires** the caller's `project_id` — the frontend can no longer read another project's review findings just by knowing/guessing a `task_id` (`listing_reviews_with_a_project_id_that_does_not_own_the_task_is_rejected`).

Tests: schema-level rejection of a mismatched `project_id` via the composite FK; schema-level rejection of `round = 3`; `PRAGMA integrity_check`/`PRAGMA foreign_key_check` both clean on a fresh v5 (and v6) database; application-level rejection of both a wrong-project write and a wrong-project read.

## 12. Major 4 — Core loop cancellation

`AppState` gained `core_loop_cancel: Arc<Mutex<HashMap<String, CancellationToken>>>` — one handle per in-flight task, inserted just before `start_core_loop` spawns the loop and removed once it returns (success, failure, or cancellation), so a stale entry can never outlive its run and a fresh `start_core_loop` for the same task is never blocked by one. New `cancel_core_loop` command signals the existing token (which Phase 1's process boundary already watches on every wait-loop iteration); cancelling a task with no active handle returns a clear error.

`core_loop.rs` now distinguishes a user cancellation from an ordinary process failure at every step (`was_cancelled` helper checking `ExecutionErrorKind::Cancelled`) — dev step, review step, and the automatic fix pass all route a cancellation to `TaskState::Cancelled` (`Actor::User`), never the generic `Failed`. A real bug was found and fixed here: Nova's own before/after git audit calls inside the review step were sharing the caller's cancellation token, so cancelling the reviewer could spuriously kill Nova's own bookkeeping git calls and misreport an integrity violation — fixed by giving Nova's own audit calls a dedicated, never-cancelled token (Section 20).

On app shutdown (`RunEvent::Exit`), every still-active core-loop token is cancelled.

**10 tests**: cancel while the mock developer AI is "running" (asserts `Cancelled`, not `Failed`, and the reviewer is never reached); cancel while the reviewer is "running" (asserts `Cancelled`, not `NeedsFix`, and a review row is still recorded as evidence); cancel during the automatic fix pass (proves `safe_fixing → cancelled` is reachable); repeated cancellation of an already-cancelled task is a harmless no-op, and a second `run_core_loop` call against the now-terminal task is rejected, not silently resumed; two projects' tasks never interfere with each other's cancellation; the Action Ledger and task state remain mutually consistent after a cancellation (the action is left `failed`, never stuck `started`).

## 13. Major 5 — Seven-document generation

New `core_loop::documents::generate_result_documents`, regenerating **all seven documents from the database's current truth every time it is called** (never incrementally patched, so they can never drift out of sync with a later state change) under `<app-data>/result-documents/<project-id>/<task-id>/` — a third app-owned root, distinct from `task-packages/` and `review-workspaces/`. Called at every exit point of `run_core_loop` (pre-start cancellation, dirty-worktree guard, and the normal end).

`REQUIREMENT.md` from the task's own confirmed fields. `DEVELOPMENT_REPORT.md`/`FIX_REPORT.md` from the Action Ledger's `dev-*-round-1`/`round-2` entries — `"（尚未触发）"` when the entry doesn't exist yet. `TEST_REPORT.md` from `test_evidence` per round, same not-yet-triggered handling. `FIRST_REVIEW.md`/`SECOND_REVIEW.md` from `task_reviews` filtered to the task's *current* `result_generation` and the matching round. `FINAL_RESULT.md` independently re-derives both the review gate (a real `approve` row for the current generation) and the test gate (a `passed`/`no_tests_applicable` row for the round that produced the approval) before it will ever say `"等待用户验收"`, and only says `"已完成"` when `task.state == Completed` **and** a real `user_acceptance` row for the current generation exists — proven never to say `"已完成"` prematurely (`final_result_never_says_completed_before_a_real_user_acceptance_exists`).

**5 tests**: all seven documents exist with explicit not-triggered status for a fresh task; `REQUIREMENT.md` reflects real task fields; `FINAL_RESULT.md` never says "已完成" before real acceptance; documents live outside the project directory under the app-owned root; regenerating after a state change (a review is recorded) produces fresh, non-stale content.

## 14. Minors

1. **`command_evidence` stdout/stderr refs**: `core_loop::record_command_evidence` now stores real, redacted, length-capped (2000 char, UTF-8-boundary-safe) summaries instead of hardcoded `None` (`command_evidence_stores_a_real_redacted_stdout_and_stderr_reference_not_always_none`).
2. **Git command failure classification**: new `GitOutputOutcome` enum (`Success`/`NotAGitRepository`/`ProjectDirectoryMissing`/`ExecutionFailed`) distinguishes these cases instead of collapsing every git failure into an undifferentiated `None`; `git_output`/`git_status_porcelain`/`git_head` derive their existing `Option<String>` behavior from it (control flow unchanged, classification gained). 3 new tests.
3. **`task_reviews.round` CHECK(1,2)**: done as part of schema v5 (Section 11).
4. Additional adversarial tests: woven throughout Sections 5–13 above rather than listed separately.

## 15. Environment-blocked real CLI verification (honest statement)

`where.exe codex`, `where.exe claude`, `Get-Command codex`, `Get-Command claude`, and direct `codex --version`/`claude --version` invocation were all attempted in this execution session. **Neither binary is installed or reachable from this shell.** No local install was found under common locations; `npm list -g --depth=0` shows neither package. This is the same finding as the prior turn's investigation, re-confirmed.

Consequently:
- The real Codex/Claude readiness probe, argv construction, and end-to-end smoke test **cannot be exercised against a genuine installed binary in this environment**.
- What **was** verified instead, honestly and thoroughly: the adapters' own logic (argv/stdin/cwd construction, readiness classification, role-swap correctness, non-zero-exit-with-embedded-"approve" rejection) against a real, separately-compiled stand-in executable (`nova_test_helper`'s `fake-cli` mode) driven through the exact same Phase 1 process boundary a genuine CLI would be — see Section 6.
- No CLI or GitHub build results have been faked. No claim is made that the real dual-AI loop has passed end to end against genuine Codex/Claude binaries.
- Shortest path for the user to unblock full verification: install the Codex CLI and the Claude Code CLI on the machine running the real verification pass, ensure both are logged in, then re-run the `readiness_probe_*`-equivalent check against the real binaries (or simply open the new "项目任务" UI tab and click "检查 Codex / Claude CLI 就绪状态").

## 16. Nova-independent test-evidence gate design

Covered in full in Section 7. Summary: auto-detection is read-only and limited to a fixed marker list; execution only ever happens through a strict allowlist; every run goes through Phase 1's safe process boundary and the Action Ledger; evidence is schema-v6 `test_evidence` with an explicit status vocabulary that cannot be confused with "AI claims passed"; a real failure blocks the reviewer entirely; `finish_success_path` independently re-verifies the gate before `preview_ready`.

## 17. App-owned task-package + path-safety design

Covered in full in Section 8.

## 18. Minimal UI operation flow

Covered in full in Section 9.

## 19. Structured JSON verdict protocol

Covered in full in Section 10.

## 20. Real bugs found and fixed during this remediation round

- **`safe_app_owned_dir` race**: concurrent callers building the same not-yet-existing nested directory could spuriously fail (`ERROR_ALREADY_EXISTS` from the losing thread's `create_dir`). Fixed by treating `AlreadyExists` from `create_dir` as "re-check what now exists" rather than a hard error, exactly mirroring the existing `Ok(metadata)` branch's symlink/is-dir checks.
- **`Running → SafeFixing` is not a legal Phase 2 transition** (only `Verifying → SafeFixing` is): the new test-execution step's early-return-on-failure path left the task in `Running` when tests failed, which then made the existing `NeedsFix` handling's `Running → SafeFixing` transition illegal and made `run_core_loop` return an unexpected `Err`. Fixed by moving the `Verifying` transition to before test execution (semantically correct too — independent testing is part of "verifying" a round), never touching the already-accepted Phase 2 transition table itself.
- **`extract_count_before` picked the wrong "N failed" occurrence**: `cargo test`'s real failure output puts a genuine `"1 failed"` summary in stdout, then a separate, countless `"error: test failed, to rerun pass `--lib`"` line in stderr; since stdout+stderr are concatenated with stderr last, a naive "last occurrence of the label wins" search picked the countless stderr occurrence and silently returned no count. Fixed by trying every occurrence from last to first and using the first one that actually has a digit before it. Caught by the real end-to-end failing-crate test, not a hand-written fixture — a genuine integration-test catch.
- **`NOVA_FAKE_CLI_FORCE` env-var design caused a real cross-test race**: the initial design forced fake-cli dispatch via a process-global environment variable, which — because Rust's test runner executes tests in parallel within one process — could leak into a concurrently-running `runtime::process` test that spawns the *same* helper binary in an unrelated mode (`marker`, `echo-argv`, etc.), causing spurious, non-deterministic failures unrelated to any actual bug. Fixed by switching to an executable-identity-based trigger (a uniquely-named hard-linked/copied file whose own stem contains `fake-cli`), which is immune to any other thread's environment mutations. Verified stable across 5+ full-suite parallel runs after the fix.
- **Shared cancellation token for Nova's own git audit calls** (Section 12): fixed by giving Nova's own before/after review-integrity audit calls a dedicated, never-cancelled `CancellationToken`, distinct from the caller's AI-process cancellation token.

## 21. New and adversarial test summary

Approximately 100+ new tests were added across `app_paths.rs` (concurrency regression), `core_loop/review_isolation.rs` (11), `core_loop/test_runner.rs` (17), `core_loop/documents.rs` (5), `runtime/verdict.rs` (17), `runtime/ai_adapter.rs` (10, including fake-executable integration tests), `core_loop.rs` (≈35 new: Blocker 4 path adversarial tests, cancellation, test-execution gate, git classification, command-evidence redaction), `storage/migrations.rs` (schema v5 + v6, 7 new), `storage/reviews.rs` (2 cross-project tests), plus 6 new frontend tests. Full per-category breakdown is in Sections 5–14 above.

## 22. Full verification commands, exit codes, and counts

All run from `D:\nova\apps\desktop\src-tauri` (Rust) / `D:\nova\apps\desktop` (frontend):

1. `cargo fmt --all -- --check` — exit 0, clean after one `cargo fmt --all` pass (12 files reformatted).
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings` — exit 0, zero warnings.
3. `cargo test --workspace --all-targets --all-features` — 352 passed; 1 pre-existing (not Phase 3) test flaked once under heavy full-suite parallel load (`runtime::process::tests::immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated`) — see Section 23.
4. `cargo build --workspace --release` — exit 0, `target/release/nova.exe` produced, no `nova_test_helper.exe` present.
5. `npm test -- --run` — 19/19 passed (4 files).
6. `npm run build` — `vue-tsc --noEmit` zero errors, `vite build` succeeds.
7. `npm run tauri build` — succeeds end to end (`beforeBuildCommand` runs `npm run build`, then `cargo build --release`, producing `D:\nova\apps\desktop\src-tauri\target\release\nova.exe`).

`cargo test --lib` (the Rust-only subset) was additionally re-run **10 times** in direct succession: 9/10 clean (352/352), 1/10 showed the same single pre-existing flaky test noted above; the identical test passes reliably (5/5) when run in isolation, confirming this is heavy-parallel-load timing sensitivity in already-accepted Phase 1 code, not a Phase 3 regression.

No `#[ignore]`d or skipped test exists anywhere in the codebase (`grep -rn "#\[ignore\]" src/ tests/` returns nothing).

## 23. Remaining known flakiness (pre-existing, not a Phase 3 regression)

`runtime::process::tests::immediate_grandchild_cancel_terminates_parent_and_grandchild_repeated` (and its timeout-variant sibling) are Phase 1 tests, unmodified by this round, that spawn real Windows processes 20 times in a tight loop and assert grandchild-process termination timing. Under the increased parallel test count this round introduces (more real-process-spawning tests than Phase 1/2 alone), this specific test has been observed to fail roughly 1 time in 10 full-workspace runs, always recovering on retry, and passing 100% reliably (5/5) when run in isolation. This is a pre-existing CPU-contention timing characteristic of the test itself, not something this remediation round introduced or should silently paper over by weakening the test.

## 24. Real CLI smoke test results

Both a Codex and a Claude smoke test against genuinely installed binaries are **environment-blocked** in this session (Section 15). No smoke test result is claimed or fabricated.

## 25. `nova.exe` build result

`npm run tauri build` succeeded; `D:\nova\apps\desktop\src-tauri\target\release\nova.exe` was produced with no bundler errors. `cargo build --workspace --release` independently confirms the same binary builds cleanly and does not include the `nova_test_helper` test-only binary (feature-gated, never built by a plain release build).

## 26. Explicit closing statements

- Phase 4 was **not** entered.
- `milestone/core-loop` was **not** created.
- Nothing was pushed (no `git push` was run; this environment has no configured remote).
- No CLI or GitHub build results were faked — every claim above that a check passed is backed by an actual command run in this session; every gap (real CLI unavailability, no live browser click-through) is reported as a gap, not glossed over.
- This work awaits Codex's independent re-review.
