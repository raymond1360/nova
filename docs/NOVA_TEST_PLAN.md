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
- SSE stream retry/interruption behavior, using a local TCP mock server (`apps/desktop/src-tauri/src/api.rs::tests`, no real SDKMAX calls):
  - Normal stream: Delta chunk(s) followed by `data: [DONE]` completes in a single request (`streams_normal_delta_and_done`).
  - A transient failure with **no** Delta received yet is retried exactly once (two requests total), with no duplicated content (`retries_once_when_temporary_failure_has_no_delta`).
  - A stream that breaks (clean close, no `[DONE]`) **after** at least one Delta was received is not retried, keeps only the already-received content, and is reported as `sse_incomplete`/interrupted (`does_not_retry_when_stream_breaks_after_delta`).
  - Malformed SSE data arriving after a Delta in the same response is not retried and does not lose or duplicate the Delta already received (`malformed_sse_after_delta_does_not_retry`).
  - A genuine transport-level read error (not a clean close) after a Delta was received is likewise not retried and preserves the received content (`read_io_error_after_delta_does_not_retry`; uses an over-declared `Content-Length` followed by a closed socket as a deterministic, non-flaky stand-in for a raw TCP RST, which is not reliably reproducible cross-platform).
  - Cancelling a stream that has not yet produced any data completes within ~500ms without waiting for the next byte or the client timeout, and does not retry (`blocking_stream_can_be_cancelled_immediately_and_does_not_retry`).
  - Cancelling a stream **after** it has already delivered a Delta does not retry and preserves the already-received content (`cancel_after_receiving_delta_preserves_content_and_does_not_retry`).
- Message-status mapping for interrupted vs. error vs. complete outcomes (`apps/desktop/src-tauri/src/commands.rs::tests`).
- SQLite persistence of a single, non-duplicated partial message body after an interrupted stream (`apps/desktop/src-tauri/src/db.rs::tests::interrupted_partial_stream_preserves_single_partial_content`).

Frontend:

- API key masking.
- Diagnostic sanitization.
- Provider config export without API key.
- Chat stream event application keeping frontend content aligned with the interrupted database state (`apps/desktop/src/chatEvents.test.ts`).
- `sendMessage()` reentrancy protection, using a mocked `invoke()` (no real SDKMAX calls) — `apps/desktop/src/stores/nova.test.ts`:
  - Two back-to-back calls with no active conversation create only one conversation and call `send_chat_message` only once; the second call is rejected while the first is still awaiting `createConversation()`.
  - A `createConversation()` failure restores `sending` to `false` and allows an immediate retry.
  - A `send_chat_message` failure restores `sending` to `false`, surfaces the (already-redacted) error message, and does not leak a raw API key pattern.
  - A normal send leaves `sending` as `true` until `finishSending()` runs (unchanged existing behavior — `sending` is not reset just because `invoke()` resolved).
  - After a user cancellation resolves via `finishSending()`, a subsequent send is accepted normally.

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
