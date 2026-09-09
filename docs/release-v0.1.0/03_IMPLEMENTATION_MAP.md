# Nova v0.1.0-beta.1 Implementation Map

Generated: 2026-09-09
Repository: `D:\nova`

This map is based on the current code, not only on product documents. Work packages are intentionally small enough to test, commit, roll back, and review independently.

## Milestone Naming Convention

Rollback references may use these local milestone names in release notes and review reports. Do not create Git tags during Stage 0 remediation.

- `milestone/runtime-foundation`: accepted end of Phase 1.
- `milestone/task-engine`: accepted end of Phase 2.
- `milestone/core-loop`: accepted end of Phase 3.
- `milestone/evidence-gate`: accepted end of Phase 4.
- `milestone/project-entry`: accepted end of Phases 5 and 6.
- `milestone/i18n-a11y`: accepted end of Phase 7.
- `milestone/release-candidate`: accepted end of Phase 8.
- `milestone/windows-release`: accepted end of Phase 9.

## Phase 1: Runtime Foundation, Environment Detection, and Safety Boundary

Goal:

- Establish the safe execution foundation before building the dual-AI workflow.
- Detect Git, Codex CLI, Claude Code CLI, app data directory, and platform readiness.
- Define a single process/command execution boundary with cancellation, timeout, redaction, and no shell-string command construction.

Likely affected files:

- `D:\nova\apps\desktop\src-tauri\src\commands.rs`
- `D:\nova\apps\desktop\src-tauri\src\models.rs`
- `D:\nova\apps\desktop\src-tauri\src\security.rs`
- `D:\nova\apps\desktop\src-tauri\src\validation.rs`
- New Rust modules under `D:\nova\apps\desktop\src-tauri\src\`
- `D:\nova\apps\desktop\src\stores\nova.ts`
- `D:\nova\apps\desktop\src\types.ts`
- `D:\nova\apps\desktop\src\App.vue`

Local tests:

- Missing Git/Codex/Claude executable.
- Installed and runnable.
- Installed but not logged in.
- Login/session invalid or expired.
- Insufficient quota.
- Insufficient permission for the requested action or directory.
- Process start failure.
- Non-zero process exit.
- Path with spaces and Chinese characters.
- Timeout and user cancellation.
- Unknown CLI error with redacted diagnostic detail.
- Redaction of Authorization, Cookie, token-like strings, and env-like secrets.
- Command injection attempts in paths/arguments.

Claude review gate:

- No command injection.
- No credentials in logs.
- Review mode cannot write.
- No access outside explicitly selected project directory.
- No automatic dependency install or third-party project execution.

Git boundary:

- One commit for environment/runner foundation only.
- Do not include UI redesign or project-entry flows in this commit.

Rollback point:

- Revert the single foundation commit back to `milestone/runtime-foundation` predecessor.

Prerequisites:

- User decision on 241 untracked files.
- Accepted scope docs committed or otherwise frozen.

Forbidden expansion:

- No GitHub clone implementation.
- No Codex/Claude task execution beyond detection and mock-safe runner tests.
- No SDKMAX server/account work.

## Phase 2: Task State Machine, Local Storage, and Interruption Recovery

Goal:

- Add explicit V3 task state model and durable local persistence.
- Support restart/resume without repeating completed destructive actions.
- Add migration backup/recovery before any non-trivial SQLite migration.

Likely affected files:

- `D:\nova\apps\desktop\src-tauri\src\db.rs`
- `D:\nova\apps\desktop\src-tauri\src\models.rs`
- New task/workflow Rust module.
- `D:\nova\apps\desktop\src\types.ts`
- `D:\nova\apps\desktop\src\stores\nova.ts`

Local tests:

- Legal and illegal state transitions.
- Restore task after app restart in each phase.
- Simulated crash while developing/reviewing/fixing.
- Migration backup failure and migration failure recovery.

Claude review gate:

- No path can skip review and enter completed.
- Recovery is idempotent.
- Failed or interrupted actions do not become completed.

Git boundary:

- One commit for schema/state/recovery foundation.

Rollback point:

- Revert commit back to `milestone/task-engine` predecessor and restore pre-migration database backup in manual test fixture.

Prerequisites:

- Phase 1 runner and path safety boundary.

Forbidden expansion:

- No real Codex/Claude execution yet except mocks.
- No UI polish beyond minimal state visibility needed for tests.

## Phase 3: Open Local Project Dual-AI Core Loop

Goal:

- Implement the smallest end-to-end core value through only "Open Local Project".
- Requirement input -> confirmation -> developer AI -> evidence collection -> read-only reviewer -> one fix pass -> second review -> user acceptance.

Likely affected files:

- Runner/adapters from Phase 1.
- Task engine from Phase 2.
- New Codex/Claude adapter modules.
- `D:\nova\apps\desktop\src\App.vue`
- `D:\nova\apps\desktop\src\stores\nova.ts`
- Markdown templates under an app-owned template location.

Local tests:

- Mock Codex success/failure.
- Mock Claude review APPROVE/CHANGES_REQUIRED/INSUFFICIENT_EVIDENCE.
- One fix pass only.
- Second review still failing stops at not completed.
- Dirty project worktree warning.
- Reviewer write attempts rejected.

Claude review gate:

- Reviewer context includes requirement, docs, diff, and test evidence.
- Reviewer is read-only by technical enforcement, not just prompt text.
- Evidence is collected independently by Nova, not trusted from AI claims.

Git boundary:

- One commit for local-project core loop.

Rollback point:

- Revert core-loop commit back to `milestone/core-loop` predecessor; no changes to user-selected sample projects should remain outside test fixtures.

Prerequisites:

- Phases 1 and 2.

Forbidden expansion:

- No create-product template.
- No GitHub import.
- No unlimited rework.
- No automatic push/PR.

## Phase 4: Timeline, Evidence Center, Markdown Docs, and Completion Gate

Goal:

- Persist and display real timeline events.
- Persist Git status/diff/file/command/test evidence.
- Generate required Markdown task docs.
- Prevent completed status until system evidence, final review, and user acceptance are all satisfied.

Likely affected files:

- `D:\nova\apps\desktop\src-tauri\src\db.rs`
- New evidence/document modules.
- `D:\nova\apps\desktop\src\App.vue`
- `D:\nova\apps\desktop\src\stores\nova.ts`
- `D:\nova\apps\desktop\src\types.ts`

Local tests:

- Command evidence records cwd, start/end time, exit code, and summary.
- Git evidence records branch, HEAD, status, and diff summary.
- Markdown docs generated under `.nova\tasks\<task-id>\`.
- Completed gate rejects missing tests, missing review, or missing user confirmation.
- Long output is summarized without leaking secrets.

Claude review gate:

- UI does not fake progress.
- Failed tests are not shown as passing.
- AI-written reports do not replace Nova-collected evidence.

Git boundary:

- One commit for evidence/docs/completion gate.

Rollback point:

- Revert evidence/docs commit.

Prerequisites:

- Phase 3 core loop.

Forbidden expansion:

- No remote upload of diagnostic data.
- No storage of private reasoning chains.

## Phase 5: Create Product Beta

Goal:

- Add one built-in Vue 3 + Vite + TypeScript template flow.
- Refuse to overwrite non-empty directories.
- Generate initial product docs and initialize Git only after user confirmation.

Likely affected files:

- New template directory.
- Project creation Rust module.
- Frontend entry UI.
- Task/document generation module.

Local tests:

- Create in empty temp directory.
- Reject existing non-empty directory.
- Template contains expected scripts, i18n structure, tests, and no backend/payment/login/database dependencies.
- Project path with spaces/Chinese characters.

Claude review gate:

- No user file overwrite.
- Template scope does not exceed V3 beta limits.

Git boundary:

- One commit for template and create-product flow.

Rollback point:

- Revert commit and delete only generated temp fixture outputs.

Prerequisites:

- Phase 4 docs/evidence system.

Forbidden expansion:

- No template marketplace.
- No complex backend template.

## Phase 6: Public GitHub Import

Goal:

- Clone public repositories into user-selected new directories.
- Read README/license/install docs and generate a manual setup guide.
- Never install dependencies or run imported code automatically.

Likely affected files:

- Git/GitHub import Rust module.
- URL validation module.
- Frontend GitHub import UI.
- Markdown guide generator.

Local tests:

- Valid public GitHub URL.
- Invalid/non-GitHub URL.
- Clone failure.
- Target directory already exists/non-empty.
- Missing README/license.
- README with dangerous install scripts is summarized, not executed.

Claude review gate:

- No third-party code execution.
- No private repo auth.
- No automatic push/PR.

Git boundary:

- One commit for GitHub import only.

Rollback point:

- Revert commit; delete only test fixture clone directories.

Prerequisites:

- Phase 1 command safety and Phase 4 evidence/docs.

Forbidden expansion:

- No private GitHub.
- No npm/pip/docker install automation.
- No running cloned project.

## Phase 7: Chinese/English UI and Accessibility

Goal:

- Replace hardcoded UI text with i18n resources.
- Provide complete Simplified Chinese and English UI for V3 surfaces.
- Improve keyboard, focus, loading, empty, and error states.

Likely affected files:

- `D:\nova\apps\desktop\src\App.vue`
- `D:\nova\apps\desktop\src\styles.css`
- New i18n resource files.
- `D:\nova\apps\desktop\src\stores\nova.ts`

Local tests:

- Language switch without restart or task loss.
- Long English/Chinese labels fit.
- Keyboard-only navigation.
- Basic contrast/focus checks.
- Error states and empty states.

Claude review gate:

- No hardcoded user-visible strings in Vue components except technical literals.
- Non-technical users can identify current status, risk, and next action.

Git boundary:

- One commit for i18n/accessibility pass.

Rollback point:

- Revert UI/i18n commit.

Prerequisites:

- Major V3 screens exist from phases 3-6.

Forbidden expansion:

- No decorative redesign that obscures task evidence.
- No copied OpenAI/Anthropic branding.

## Phase 8: Release Candidate and Local End-to-End Verification

Goal:

- Freeze a release candidate from the completed V3 beta workflow.
- Run local end-to-end scenarios from `D:\nova\product\NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md` Phase 7 before CI/release packaging.
- Produce release candidate evidence and known-limitations documentation without adding new product scope.

Likely affected files:

- Release candidate docs under `D:\nova\docs\release-v0.1.0\`.
- Existing verification script/reporting surfaces, if evidence capture needs a documented local command.
- No application behavior change unless an E2E scenario exposes a blocking defect that is fixed in a separate scoped commit.

Local E2E scenarios:

- First launch on Windows with no Nova account.
- Git installed and detected as ready.
- Git missing or not found on PATH.
- Codex CLI installed and runnable.
- Codex CLI installed but not logged in, session invalid, quota-limited, permission-denied, start-failed, non-zero exit, timeout, user-cancelled, and unknown-error states.
- Claude Code CLI installed and runnable.
- Claude Code CLI installed but not logged in, session invalid, quota-limited, permission-denied, start-failed, non-zero exit, timeout, user-cancelled, and unknown-error states.
- Open local project with a clean worktree.
- Open local project with uncommitted user work and verify Nova blocks unsafe modification until the user confirms.
- Create product beta in an empty target directory.
- Reject create-product beta in a non-empty target directory.
- Import a public GitHub repository into a new target directory without installing dependencies or running imported code.
- Run the fixed workflow: requirement confirmation -> developer AI -> first review -> one fix pass -> second review -> user acceptance.
- Run the role-swapped fixed workflow: requirement confirmation -> Claude Code developer -> Codex read-only reviewer -> one fix pass -> second review -> user acceptance.
- Run the second-review-fails path and verify Nova stops for user decision instead of starting another automatic rework loop.
- Restart Nova during draft, development, review, fix, and user-acceptance states and verify recovery does not duplicate destructive actions.
- Switch Simplified Chinese / English without losing task state, input, timeline, or scroll position.
- Verify generated Markdown task documents and evidence records match the real Git status, diff, command exits, and test results.

Release docs:

- `RC_CHECKLIST.md`: release candidate checklist with scenario owner, pass/fail status, evidence path, and exact commit.
- `RC_E2E_EVIDENCE.md`: local E2E evidence for every required scenario, including the default and role-swapped fixed workflows and the second-review-fails path.
- `KNOWN_LIMITATIONS.md`: known limitations that do not contradict P0 scope.
- `ROLLBACK_NOTES.md`: manual rollback notes.
- `CHECKSUM_PLAN.md`: SHA256/checksum plan for installer artifacts.
- `CLAUDE_RC_REVIEW_PACKET.md`: Claude review packet containing scope, gap matrix, implementation map, E2E evidence, release docs, and the exact candidate commit.

Claude review gate:

- Claude must review the local E2E evidence and release candidate docs before packaging/release work starts.
- Any Blocker or Major finding returns to the smallest relevant prior phase; do not advance to CI/release packaging with open Blocker/Major findings.

Git boundary:

- One commit for release candidate evidence/docs only.
- Defect fixes found during this phase must be separate, scoped commits against the affected feature phase.

Rollback point:

- Return to `milestone/release-candidate` predecessor or the last accepted milestone before the failing E2E scenario.

Prerequisites:

- Phases 1-7 complete and reviewed.
- No uncommitted user work in the Nova repository except deliberate release-candidate evidence.
- Accepted Stage 0 baseline commit and subsequent phase commits available in history.

Forbidden expansion:

- No new product features.
- No automatic production deployment.
- No private GitHub authorization.
- No extra rework loops beyond the fixed beta workflow.

Pass criteria:

- Every local E2E scenario has recorded evidence, pass/fail status, and exact commit identifier, including role-swapped execution and second-review-fails handling.
- No Blocker/Major Claude findings remain open.
- Known limitations are documented and do not contradict P0 scope.

## Phase 9: Windows Build, CI, and Release Preparation

Goal:

- Add GitHub Actions on Windows.
- Define release candidate checks, installer artifact generation, SHA256, rollback docs, and known limitations.

Likely affected files:

- `D:\nova\.github\workflows\...`
- `D:\nova\scripts\verify-build.ps1`
- `D:\nova\apps\desktop\src-tauri\tauri.conf.json`
- Release docs under `D:\nova\docs\release-v0.1.0\`

Local/CI tests:

- npm test/build.
- Rust fmt/clippy/test.
- Tauri production build.
- Installer launch on clean Windows VM.
- SHA256 verification.

Claude review gate:

- CI artifact corresponds to audited commit/tag.
- No secrets in logs/artifacts.
- Known limitations are public.

Git boundary:

- One commit for CI/release wiring.
- Release tag only after final user approval.

Rollback point:

- Previous audited commit and previous installer artifact, if any.

Prerequisites:

- Remote repository configured.
- User-approved release branch/tag policy.
- All Blocker/Major issues closed.
- Phase 8 release candidate and local E2E verification passed.

Forbidden expansion:

- No automatic production deployment.
- No auto-update system in v0.1.0.
- No signing claims unless certificate is actually configured and verified.

## Recommended Phase 1 Minimum Work Package

Do only this next:

Runtime foundation, environment detection, and safety boundary.

Acceptance target:

- Detect Git, Codex CLI, and Claude Code CLI states.
- Add a safe process runner with argument-array execution, timeout, cancellation, redaction, and working-directory boundary checks.
- Add tests for missing tools, path edge cases, cancellation, timeout, and redaction.
- Do not implement project creation, GitHub import, or real dual-AI workflow yet.
