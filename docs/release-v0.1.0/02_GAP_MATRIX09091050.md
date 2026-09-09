# Nova v0.1.0-beta.1 Gap Matrix

Generated: 2026-09-09
Baseline: `D:\nova\product\NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md`

Status legend:

- Complete: implemented and evidenced in current code/config/logs.
- Partial: some foundation exists, but V3 behavior is missing or incomplete.
- Missing: no implementation found.
- Conflict: implementation exists but conflicts with V3.
- Unverified: cannot be confirmed by static audit and existing logs.

| Requirement | Current status | Evidence | Gap | Risk | Suggested phase | Acceptance method |
| --- | --- | --- | --- | --- | --- | --- |
| Windows desktop client | Partial | `D:\nova\apps\desktop\src-tauri\tauri.conf.json`, `test-results\verify-build-tauri-build.log` | Prior build evidence exists; no CI/release installer workflow | Major | 8 | Windows clean-machine install and launch, GitHub Actions artifact |
| No Nova registration | Partial | Current app stores SDKMAX provider config, no Nova login UI in `App.vue` | Existing UI still requires user-provided SDKMAX API key for chat | Minor | 1 | Launch without Nova account and use non-network flows |
| Chinese and English UI | Missing | `App.vue` has hardcoded Chinese; no i18n files found | No i18n resource system or English UI | Major | 7 | Switch language without restart or task loss |
| Git/Codex/Claude CLI detection | Missing | No command/service references found for Codex or Claude adapters | No environment readiness panel or complete CLI status model; Phase 1 cannot rely only on `--version` | Blocker | 1 | Simulated installed, missing, login, permission, quota, start failure, non-zero exit, timeout, cancellation, and unknown-error states with tests |
| Three project entries | Missing | `App.vue` tabs are chat/providers/settings | No create/open/import entry screen | Blocker | 5 or split after phase 3 | UI and command tests for all three entries |
| Create product beta from one built-in Vue/Vite/TS template | Missing | No template directory/service found | No template copy, target-dir guard, Git init, or template version | Major | 5 | Create sample project in empty temp dir, reject non-empty dir |
| Open local project | Missing | No directory picker/project service found | No Git/status/rules scan for chosen project | Major | 3 then 5 | Mock and real local sample project tests |
| Import from public GitHub | Missing | No clone/GitHub URL import service found | No URL validation, clone isolation, README/license parsing, manual guide generation | Major | 6 | Public repo clone test; prove no install/run commands execute |
| Natural language requirement input | Partial/conflict | Current chat input in `App.vue` sends SDKMAX chat messages | It is generic chat, not structured development request intake | Major | 3 | Requirement input creates a task draft, not a chat-only message |
| Requirement clarification and confirmation | Missing | No confirmation card/state found | No scope/non-goals/acceptance criteria confirmation | Blocker | 3 | User must approve requirement card before any development command |
| Modification-before-protection/user work guard | Missing | Git status can be read manually, but no app logic | No guard against overwriting uncommitted user changes | Blocker | 1 and 3 | Dirty worktree blocks task start until user confirms |
| Codex development adapter | Missing | No Codex CLI command execution code | No structured task package, process execution, output parsing, cancellation | Blocker | 1 and 3 | Mock adapter tests, then controlled real smoke test |
| Claude Code read-only review adapter | Missing | No Claude CLI command execution code | No read-only enforcement or review prompt package | Blocker | 1 and 3 | Reviewer process cannot write in test fixture |
| One fix pass and second review | Missing | No workflow engine found | No automated handoff after first review | Blocker | 3 | Failing first review produces one fix package and second review |
| Role swap before task start | Missing | No role state in frontend/types/db | Default/alternate role model absent | Major | 3 | Role can swap before start and locks after start |
| Explicit task state machine | Missing | Current message status is only chat message status | No V3 states such as DRAFT, FIRST_REVIEW, FIXING, USER_ACCEPTANCE | Blocker | 2 | Unit tests for legal/illegal transitions |
| Interruption recovery | Partial | SQLite conversation restore exists in `db.rs` tests | Chat interruption handled; V3 task recovery absent | Major | 2 | Kill/restart during each task phase and resume safely |
| Real task timeline | Missing | Current UI shows messages only | No event model for task actions, waits, commands, evidence | Major | 4 | Timeline persists event timestamps and statuses |
| Evidence center | Partial | SDKMAX request logs exist in `db.rs`; build logs exist under `test-results` | No V3 evidence model for Git diff/files/commands/tests | Blocker | 4 | Evidence records include command, cwd, times, exit code, artifacts |
| Markdown task documents | Missing | No `.nova\tasks\<task-id>` generator found | Required seven task docs absent | Major | 4 | Generated docs match template and link to evidence |
| User completion confirmation | Missing | No acceptance state/UI | App can finish chat but not user-gated task completion | Blocker | 4 | Cannot enter COMPLETED without user acceptance plus evidence |
| SSE/stream integrity | Complete for current chat MVP | `api.rs:24`, `api.rs:138`, `api.rs:141`, tests in `api.rs` | Applies to SDKMAX chat, not yet to Codex/Claude CLI streams | Minor | 1 | Reuse/adapt stream tests for CLI adapters |
| Stop/cancel stream | Complete for current chat MVP | `api.rs:185`, `commands.rs:317`, tests in `api.rs` | Needs per-task/per-process cancellation for V3 adapters | Minor | 1 | Cancel each adapter and preserve partial logs |
| SQLite local persistence | Partial | `db.rs` tables for chat/provider/request logs | No V3 task/evidence/document schema, no migration backup | Major | 2 | Migration backup and recovery tests |
| API key security | Complete for current chat MVP | `credential.rs`, `security.rs`, `db.rs` | V3 should minimize need for SDKMAX-specific provider UI if out of scope | Minor | 1 | Secret scans and no plaintext key in DB/log/UI store |
| Endpoint allowlist | Complete for current chat MVP | `validation.rs` | V3 GitHub/Codex/Claude flows need separate command allowlist | Minor | 1 | Allowed command/URL tests |
| Git evidence | Missing | No app Git service found | No diff/status/HEAD capture per task | Blocker | 4 | Fixture repo tests for clean/dirty/diff/status |
| Test evidence | Missing | Existing build logs are manual/project-level | No task-level command evidence capture | Major | 4 | Command executor stores stdout/stderr summary and exit code |
| GitHub Actions | Missing | No `D:\nova\.github\workflows` found | No CI gate for PR/release | Major | 8 | Workflow runs npm/Rust/Tauri checks on Windows |
| Windows installer/package | Partial | Tauri NSIS target declared but `bundle.active=false` | No release pipeline; no signed/unsigned release policy in CI | Major | 8 | Release candidate produces artifact and SHA256 |
| Product docs consistency | Partial/conflict | `product\*.md`, older `docs\*.md` | Older SDKMAX chat MVP docs conflict with V3 scope | Major | 0/1 | Archive/label old docs or commit accepted scope docs |
| Historical Claude issues closure | Partial | Historical reports plus current code show fixes | Real SDKMAX manual integration remains unverified | Minor/Major depending release gate | 1/8 | User-provided credential/manual smoke evidence, if in scope |

## Summary by Category

Complete in current code:

- Current SDKMAX chat stream integrity.
- Current chat cancellation handling.
- Current API-key masking/redaction and Credential Manager storage.
- Current SDKMAX endpoint allowlist.
- CSP and limited Tauri capabilities.

Partially reusable:

- Tauri/Vue/Rust/SQLite app skeleton.
- SQLite migration pattern.
- Request logging pattern.
- Build/test verification script and historical logs.
- Frontend state store and message persistence.

Missing for V3 P0:

- Almost all dual-AI development/review workflow behavior.
- All three project entry flows.
- CLI adapter layer.
- Task state machine.
- Evidence center and Markdown task document generation.
- User acceptance gate.
- i18n.
- CI/release pipeline.

## Required CLI Status Model

Phase 1 environment detection must treat CLI readiness as a structured state model, not a simple `--version` check. At minimum it must distinguish:

- Not installed or not found on PATH.
- Installed and runnable.
- Installed but not logged in.
- Login/session invalid or expired.
- Insufficient quota.
- Insufficient permission for the requested action or directory.
- Process start failure.
- Non-zero process exit.
- Timeout.
- User cancellation.
- Unknown error with redacted diagnostic detail.

The UI and task engine must preserve the exact state so Nova can choose the correct next action: show installation guidance, ask the user to complete official login, block task start, offer retry, or record an interrupted task without pretending it completed.

Conflicts:

- Current UI and older docs present Nova as SDKMAX chat/API-key MVP, not the V3 dual-AI development/review client.
- Repository currently has a large untracked baseline, unsuitable as a clean development start.
