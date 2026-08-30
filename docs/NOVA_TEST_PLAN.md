# Nova Test Plan

## Implemented Automated Coverage

Rust:

- Base URL validation and normalization.
- Path allowlist rejection for admin SDKMAX APIs.
- API key masking.
- Authorization, Cookie, and key-like diagnostic redaction.
- `/v1/models` response parsing.
- SSE cross-chunk parsing.
- SSE `[DONE]` handling.
- SSE malformed JSON detection.
- HTTP error classification for 401, 403, 429, `insufficient_user_quota`, `model_not_found`, and `get_channel_failed`.
- SQLite repeatable migration.
- Conversation save and restore.
- Provider metadata without raw API key.
- Credential save/read/delete through test credential store.

Frontend:

- API key masking.
- Diagnostic sanitization.
- Provider config export without API key.

## Manual Acceptance Checks

- `npm run tauri dev` opens Nova.
- Save a key, restart, and verify the provider still has `has_key` without showing the full key.
- Refresh `/v1/models`, select a model, and stream a short message.
- Stop generation and confirm partial output is retained as interrupted.
- Verify SQLite and logs contain no full key.

Manual real SDKMAX calls require user confirmation and a key from Windows Credential Manager or environment, never from a committed `.env` file.

## CSP Verification (Manual)

- Run `npm run tauri -- build`, launch the resulting `apps\desktop\src-tauri\target\release\nova.exe`, and confirm the window renders with full styling (no blank/unstyled UI, which would indicate the production CSP in `tauri.conf.json` blocked a required resource).
- Type a message containing `<script>alert(1)</script>` and confirm it renders as inert text (no dialog fires). This must not require a real SDKMAX API call — the check applies to how the message is rendered in the input/composer or as already-stored local content, not to a live chat completion.
- No informational or authorization-related content should be required to perform this check.

## Build/Commit Verification

`scripts\verify-build.ps1` automates the four commands above (`npm test`, `npm run build`, `cargo test`, `npm run tauri -- build`) and binds their pass/fail result to the Git commit hash under test in `test-results\build-info.txt`. See `NOVA_DEVELOPMENT_WORKFLOW.md` for the commit-then-verify workflow. This does not replace the manual acceptance checks above (which need a running window and human observation); it only replaces manually re-running and eyeballing the four commands' exit codes.
