# Nova v0.1.0-beta.1 Baseline Audit

Generated: 2026-09-09
Repository: `D:\nova`

## Executive Conclusion

The current repository is not ready to enter product-feature development for V3 `v0.1.0-beta.1` without first cleaning or explicitly accepting the working-tree baseline. The tracked application is a Tauri/Vue/Rust SDKMAX chat MVP with SQLite persistence and SDKMAX API integration. It is not yet the V3 Nova dual-AI development/review workflow product.

Estimated V3 completion: low to early partial. Foundational technical pieces exist, especially Tauri, Vue, Rust, SQLite, credentials, streaming, cancellation, and tests. The main V3 user workflows are mostly absent.

## Git Baseline

- Git repository: yes. Evidence: `git rev-parse --is-inside-work-tree` returned `true`.
- Current branch: `feature/nova-technical-mvp`.
- Current HEAD: `e1e967548621b78c2b9a78e2e08b43159c3040be`.
- Latest commits:
  - `e1e9675 fix: prevent concurrent chat submission`
  - `465d3c8 fix: preserve stream integrity and cancellation`
  - `e0de353 chore: harden CSP and build traceability`
  - `3e1d2dd feat: add nova technical mvp`
  - `e0f1a7b chore: initialize nova audit baseline`
- Branches: local `feature/nova-technical-mvp` and `master`; no `develop` branch observed.
- Remotes: none printed by `git remote -v`; no remote repository is currently configured.
- Tags: none printed by `git tag --list --sort=-creatordate`.
- Upstream tracking: none shown by `git branch -vv`; current branch is local-only.
- Pre-remediation working tree: no modified tracked files were shown, but 241 untracked files were listed by `git ls-files --others --exclude-standard`.

Untracked material includes:

- `D:\nova\.cloude\...`
- `D:\nova\NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`
- `D:\nova\audit\CLAUDE_REVIEW_NOVA_TECHNICAL_MVP.md`
- `D:\nova\audit\CLAUDE_REVIEW_NOVA_TECHNICAL_MVP_FINAL.md`
- `D:\nova\docs\chatgpt.txt`
- `D:\nova\docs\product\NOVA_V1_PRODUCT_DESIGN_PLAN.md`
- `D:\nova\nova-ui-design.html`
- `D:\nova\product\NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`
- `D:\nova\product\NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`

Risk: formal development should not begin until the user decides whether these untracked files are intended inputs to preserve, files to commit, or files to move out of the repo. Stage 0 remediation classifies this material in `D:\nova\docs\release-v0.1.0\STAGE0_REMEDIATION_REPORT09091050.md`.

## Repository Structure

Observed top-level structure:

- `D:\nova\apps\desktop\`: Tauri desktop app with Vue frontend and Rust backend.
- `D:\nova\audit\`: historical Claude/SDKMAX audit reports.
- `D:\nova\docs\`: earlier product, architecture, security, workflow, test, and technical debt docs.
- `D:\nova\product\`: current V3 product and minimum release plan.
- `D:\nova\scripts\verify-build.ps1`: build/test verification helper.
- `D:\nova\services\`: present but no active service implementation observed in this audit.
- `D:\nova\test-results\`: prior test/build logs and `build-info.txt`.

No repository root `.github\workflows` directory was found. `.cloude\skills\claude-code-main\.github\workflows\...` exists but is untracked skill/example material, not project CI.

## Technology Baseline

Frontend:

- Vue 3.5 line, from `D:\nova\apps\desktop\package.json`.
- Vite 6 line, from `D:\nova\apps\desktop\package.json`.
- TypeScript 5 line, from `D:\nova\apps\desktop\package.json`.
- Pinia 2 line, from `D:\nova\apps\desktop\package.json`.
- Vitest 2 line, from `D:\nova\apps\desktop\package.json`.
- Actual lockfile examples: Vue `3.5.42`, Vite `6.4.3`, TypeScript `5.9.3`, Vitest `2.1.9`, Tauri CLI `2.11.4`, Tauri API `2.11.1`, Pinia `2.3.1`.

Backend/desktop:

- Tauri 2, evidence: `D:\nova\apps\desktop\src-tauri\Cargo.toml` and `tauri.conf.json`.
- Rust 2021 edition, evidence: `D:\nova\apps\desktop\src-tauri\Cargo.toml`.
- SQLite via `rusqlite` with bundled SQLite, evidence: `D:\nova\apps\desktop\src-tauri\Cargo.toml`.
- HTTP via `reqwest`, async runtime via `tokio`, cancellation via `tokio-util`, evidence: `Cargo.toml`.
- Windows Credential Manager integration via `windows-sys`, evidence: `credential.rs`.

## Main Entry Files

- Frontend entry: `D:\nova\apps\desktop\src\main.ts`.
- Primary Vue shell: `D:\nova\apps\desktop\src\App.vue`.
- Frontend state: `D:\nova\apps\desktop\src\stores\nova.ts`.
- Tauri command registration: `D:\nova\apps\desktop\src-tauri\src\lib.rs`.
- Tauri commands: `D:\nova\apps\desktop\src-tauri\src\commands.rs`.
- SDKMAX API streaming and parsing: `D:\nova\apps\desktop\src-tauri\src\api.rs`.
- SQLite storage: `D:\nova\apps\desktop\src-tauri\src\db.rs`.
- Credential storage: `D:\nova\apps\desktop\src-tauri\src\credential.rs`.
- Security helpers: `D:\nova\apps\desktop\src-tauri\src\security.rs` and `D:\nova\apps\desktop\src\security.ts`.
- URL/endpoint validation: `D:\nova\apps\desktop\src-tauri\src\validation.rs`.

## Existing Implemented Capabilities

Implemented or substantially present:

- Tauri desktop shell with a Vue UI.
- SDKMAX provider configuration UI.
- API key save/delete flow through Rust commands.
- API key storage through Windows Credential Manager on Windows and a memory store in tests/non-Windows code paths.
- SQLite tables for providers, app settings, conversations, messages, and request logs. Evidence: `db.rs` creates `providers`, `app_settings`, `conversations`, `messages`, and `request_logs`.
- Local conversation list and message persistence.
- SDKMAX model list fetch, token usage fetch, and streaming chat request.
- SSE parser and streaming event handling.
- Cancellation path with `CancellationToken`.
- Partial stream handling with `received_delta`; incomplete streams after content are treated as interrupted.
- Frontend guard against concurrent chat submission. Evidence: `stores\nova.ts:123` and `stores\nova.ts:129`; tests in `stores\nova.test.ts`.
- CSP is configured in `tauri.conf.json`.
- Tauri capabilities are limited to `core:default`.

Not implemented for V3:

- Git, Codex CLI, and Claude Code CLI environment detection.
- Three V3 project entry flows.
- Built-in Vue/Vite/TypeScript product template creation.
- Public GitHub clone/import flow and manual install guide generation.
- Requirement confirmation card.
- Dual-AI role configuration and role swap.
- Codex/Claude CLI adapters.
- Read-only reviewer enforcement at process/sandbox level.
- Fixed V3 task state machine.
- Evidence center for Git diff, commands, tests, file changes, and exit codes.
- Markdown task document generation under `.nova\tasks\<task-id>\`.
- User acceptance/completion gate.
- Full Chinese/English i18n resources.
- Windows installer/release workflow and project CI.

## Current UI Baseline

`D:\nova\apps\desktop\src\App.vue` exposes three tabs:

- Chat
- Providers
- Settings

This conflicts with V3's required first screen: create product beta, open local project, and import from GitHub. The current UI text is hardcoded Chinese in Vue components and does not use an i18n resource system.

## Storage and Recovery

Actual storage:

- SQLite database path is under Tauri app data as `nova.sqlite3`, evidence: `D:\nova\apps\desktop\src-tauri\src\lib.rs:46`.
- Database migration uses `CREATE TABLE IF NOT EXISTS`, evidence: `D:\nova\apps\desktop\src-tauri\src\db.rs:38`.
- Conversation/message restore is covered by `db::tests::migrations_are_repeatable_and_conversation_restores`.

Gaps:

- V3 task persistence, task state machine, evidence index, document index, and interruption resume workflow are not implemented.
- SQLite migration backup/failed migration recovery is not implemented beyond idempotent current-table creation.

## CLI and Process Baseline

Actual external-process behavior:

- No Codex CLI adapter found.
- No Claude Code CLI adapter found.
- No Git command/evidence service found.
- No project command runner for arbitrary project builds/tests found.
- Current subprocess-like concern is SDKMAX HTTP streaming plus cancellation, not local CLI orchestration.

## Test and Build Baseline

Declared commands from real config:

- Root `D:\nova\package.json`:
  - `npm run dev`
  - `npm run build`
  - `npm run test`
  - `npm run test:rust`
  - `npm run test:all`
  - `npm run tauri`
- Desktop `D:\nova\apps\desktop\package.json`:
  - `npm run dev`
  - `npm run build`
  - `npm run preview`
  - `npm run tauri`
  - `npm run test`
- Rust:
  - `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml`
  - `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings` are referenced by historical audit/build logs, not package scripts.
- Verification helper:
  - `powershell -File D:\nova\scripts\verify-build.ps1`

Existing test files:

- `D:\nova\apps\desktop\src\security.test.ts`
- `D:\nova\apps\desktop\src\chatEvents.test.ts`
- `D:\nova\apps\desktop\src\stores\nova.test.ts`
- Rust unit/integration-style tests embedded under `D:\nova\apps\desktop\src-tauri\src\*.rs`

Existing test log evidence, not rerun in this phase:

- `D:\nova\test-results\build-info.txt` records commit `e1e967548621b78c2b9a78e2e08b43159c3040be`, branch `feature/nova-technical-mvp`, Node `v24.14.0`, npm `11.9.0`, rustc `1.98.0`, cargo `1.98.0`, and all four verification steps PASS.
- `D:\nova\test-results\verify-build-npm-test.log` records 3 Vitest files and 10 tests passed.
- `D:\nova\test-results\verify-build-cargo-test.log` records 23 Rust tests passed.
- `D:\nova\test-results\verify-build-npm-build.log` and `verify-build-tauri-build.log` exist and are referenced from `build-info.txt`.

This phase did not rerun tests or builds because such commands can write `dist\`, `target\`, logs, or other artifacts.

## CI and Release Baseline

- No active project GitHub Actions workflow found under `D:\nova\.github\workflows`.
- No remote configured, so no remote CI or release target can be inferred.
- Tauri bundle config has `bundle.active: false` and `targets: ["nsis"]`, evidence: `D:\nova\apps\desktop\src-tauri\tauri.conf.json:27`.
- Windows production build has prior local log evidence, but no project CI release pipeline exists in the tracked repository.

## Security and Privacy Baseline

Positive evidence:

- API keys are intended to be stored outside SQLite in Windows Credential Manager. Evidence: `credential.rs` uses target `com.sdkmax.nova/{provider_id}/api-key` and Windows credential APIs.
- SQLite stores provider metadata with masked key/has_key only. Evidence: `db.rs:120`.
- Redaction exists for Authorization bearer values, Cookie values, and `sk-...`-like strings. Evidence: `security.rs:30` and `src\security.ts:8`.
- SDKMAX endpoints are allowlisted to `/v1/models`, `/v1/chat/completions`, and `/api/usage/token/`. Evidence: `validation.rs:6`, `validation.rs:16`, `validation.rs:67`.
- Production `base_url` is restricted to `https://api.sdkmax.com`; localhost HTTP is debug-only. Evidence: `validation.rs:35` and `validation.rs:45`.
- Tauri capability file only allows `core:default`. Evidence: `src-tauri\capabilities\default.json:6`.
- No `localStorage` or `sessionStorage` use was found by text search in the app source.

Risks/gaps:

- No implemented protection yet for V3 reviewer read-only filesystem boundaries, because Codex/Claude process adapters do not exist.
- No implemented project-directory boundary for opened/imported projects, because project entry flows do not exist.
- No implemented GitHub clone safety flow.
- No implemented diagnostic export workflow.
- Existing untracked `.cloude\` content includes many third-party/plugin/example files and should not be treated as project source without user decision.

No real secrets were copied into this report. Search hits for key/token terms were in source redaction code, tests, docs, or placeholder examples.

## Historical Claude Audit Status

Historical reports read:

- `D:\nova\audit\CLAUDE_REVIEW_NOVA_TECHNICAL_MVP.md`
- `D:\nova\audit\CLAUDE_REVIEW_NOVA_TECHNICAL_MVP_FINAL.md`

Issue status:

- SSE incomplete stream retry/data pollution: fixed with evidence. Current code has `received_delta`; retry only when no delta was received. Evidence: `api.rs:24`, `api.rs:138`, `api.rs:141`, tests at `api.rs:689`, `api.rs:702`, `api.rs:749`, `api.rs:766`.
- Interrupted/cancel status: fixed with evidence. Evidence: `commands.rs:298`, `commands.rs:309`, tests in `commands.rs:368` and `api.rs:714`.
- Non-preemptive cancellation: fixed with evidence. Evidence: `api.rs:185` uses `tokio::select!`; test `blocking_stream_can_be_cancelled_immediately_and_does_not_retry`.
- Production CSP unset: fixed with evidence. Evidence: `tauri.conf.json:23`.
- Build traceability: fixed with evidence in script and logs. Evidence: `scripts\verify-build.ps1` and `test-results\build-info.txt`.
- Concurrent send R1 from final review: fixed with evidence by current HEAD `e1e9675`; evidence: `stores\nova.ts:123`, `stores\nova.ts:129`, and `stores\nova.test.ts:39`.
- F5 / `active_cancel` remains a known architectural limitation, not a completed fix. `AppState.active_cancel` is still one global cancellation slot. The current UI-level single-send guard makes that acceptable only because v0.1.0 currently does not allow concurrent execution. This is documented as TD-1 in `D:\nova\docs\NOVA_TECHNICAL_DEBT.md`; before any future concurrent tasks, multiple windows, background generation, or overlapping streams are enabled, `active_cancel` must be redesigned as a keyed cancellation map.
- Real SDKMAX manual integration: still not verified in repository evidence; historical final review also listed it as external manual gate.
- V3 dual-AI product scope: not covered by historical Technical MVP; still largely absent.

## Major Baseline Risks

Blocker:

- V3 core product workflow is not implemented; current app is an SDKMAX chat MVP.

Resolved Stage 0 blocker:

- Pre-remediation working tree contained 241 untracked files, including current product docs and audit reports. This was resolved by the Stage 0 remediation baseline: accepted product/audit documents were committed, unrelated third-party or duplicate material was backed up and moved out after verification, and the post-baseline working tree was recorded as clean in `D:\nova\docs\release-v0.1.0\STAGE0_BASELINE_COMMIT_EVIDENCE09091050.md`.

Major:

- No Codex/Claude/Git environment detection or process adapters.
- No V3 task state machine, evidence center, Markdown task generation, or task recovery.
- No active project CI/release workflow.
- No full i18n implementation.

Minor:

- Historical test/build logs exist but were not rerun in this phase.
- Tauri bundling is configured with NSIS target but `bundle.active` is false.
- Older docs conflict with current V3 scope and need archival or explicit labeling.
- No remote repository is configured. This blocks remote backup, CI, release artifact publication, and pull-request review flows until an explicit remote strategy is chosen.

Suggestion:

- Commit or otherwise freeze the accepted product/audit docs before Phase 1.
- Keep Phase 1 narrow: runtime skeleton, environment detection, command safety boundary, and no user-facing dual-AI workflow yet.
