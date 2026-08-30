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
