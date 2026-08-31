# Nova Technical Debt Register

This document tracks known design shortcuts that are acceptable for the
current MVP scope but must be revisited before the product surface grows
past what they were designed for. It does not change any behavior by
itself; it is a planning artifact for future work.

## TD-1: `active_cancel` is a single global cancellation slot

- **Status**: Accepted for Nova 0.1 MVP. Not fixed in this change.
- **Owner of the implementation**: Codex (do not modify `active_cancel` or
  the cancellation flow without going through the same review process as
  F1/F2 in `audit/CLAUDE_REVIEW_NOVA_TECHNICAL_MVP.md`).

### Current design

`AppState.active_cancel` in `apps/desktop/src-tauri/src/lib.rs` is a single
`Arc<Mutex<Option<CancellationToken>>>`. Every call to `send_chat_message`
in `apps/desktop/src-tauri/src/commands.rs` overwrites whatever token is
currently stored there, and `cancel_active_chat` only ever cancels
whichever token happens to be in that slot at the moment it is called.

### Why this is acceptable today

Nova 0.1's UI (`apps/desktop/src/App.vue` + `apps/desktop/src/stores/nova.ts`)
only exposes a single active chat surface: while `store.sending` is `true`
the "发送" (send) button is replaced by a "停止" (stop) button, so the
current frontend has no code path that can issue a second
`send_chat_message` while one is already in flight. Under that constraint,
a single global slot behaves identically to a per-request slot, because
there is only ever at most one request to track.

**This invariant previously had a gap and has since been closed at the
frontend level (see "R1 fix" below); it is still enforced by the frontend
alone, not by the Rust side.** `AppState.active_cancel` itself has **not**
been changed and remains a single global slot — this section documents a
UI-level guard that makes the existing single-stream assumption actually
hold, not a change to the underlying architecture. Concurrent generation
is still not supported and is not the goal of the fix described below.

#### R1 fix: `sendMessage()` reentrancy guard (closed, frontend-only)

`CLAUDE_REVIEW_NOVA_TECHNICAL_MVP_FINAL.md` (finding R1) found that
`stores/nova.ts::sendMessage()` set `this.sending = true` **after**
`await this.createConversation()` when starting the first message of a
brand-new conversation. During that `await`, a second `sendMessage()` call
(e.g. a double `Ctrl+Enter`, or a button click racing a keyboard shortcut)
could also see `activeConversationId` as still empty, create a second
conversation, and start a second `send_chat_message` — both streams would
then fight over the single `active_cancel` slot, so "停止" could only ever
stop the newer of the two.

This has been fixed **entirely in `sendMessage()`**, by:

1. Checking `if (this.sending) return` as the very first statement (before
   any `await`, including `createConversation()`), so a second call made
   while the first is still in its synchronous prologue is rejected
   immediately.
2. Setting `this.sending = true` immediately after that guard — still
   before the first `await` — so the invariant holds even across the
   `createConversation()` gap that caused the original bug.
3. Only resetting `this.sending = false` inside `sendMessage()` itself when
   `createConversation()` or the `send_chat_message` invoke call fails
   (i.e. Rust never spawned a background stream, so no `chat://done` /
   `chat://error` will ever arrive to release the flag via
   `finishSending()`). Once the invoke call succeeds, `sendMessage()` does
   not touch `sending` again — ownership passes to `finishSending()`
   (triggered by the `chat://done`/`chat://error` listeners in `App.vue`),
   exactly as before this fix.

This closes the specific race described above. It does **not** change
`active_cancel`, does **not** allow more than one stream to run at a time,
and does **not** touch `stream_chat`/`stream_chat_once` in `api.rs`. The
architectural debt below (multiple concurrent streams, multiple windows,
background generation) is unaffected and still requires the `HashMap`
redesign before any of those features can ship.

### When this must be fixed

Before shipping any of the following, `active_cancel` must be redesigned:

- Multiple conversations streaming concurrently (e.g. a "send in background,
  switch conversation, send again" UX).
- Multiple OS windows/webviews sharing one `AppState`.
- Any background/automatic generation (retries, scheduled prompts, etc.)
  that can overlap with a user-initiated send.

Shipping any of the above while `active_cancel` stays a single slot would
mean: starting a second stream silently orphans the first one's cancel
token, so the "停止" button would stop the *newest* stream instead of the
one the user actually meant to stop. This is a UX/correctness bug, not a
security issue, but it would ship silently since nothing crashes.

### Recommended fix (not implemented yet)

Replace the single `Option<CancellationToken>` with a keyed collection,
e.g.:

```rust
active_cancels: Arc<Mutex<HashMap<String, CancellationToken>>>, // key: assistant_message_id (or conversation_id)
```

Requirements for the fix, to avoid trading one bug for another:

1. **Insertion**: `send_chat_message` inserts the new token keyed by the
   `assistant_message_id` it just created, before spawning the streaming
   task.
2. **Cancellation**: `cancel_active_chat` must take a key (assistant message
   id or conversation id) from the caller instead of implicitly cancelling
   "whatever is active" — the frontend already knows which
   conversation/message it wants to stop.
3. **Cleanup on every terminal outcome**: the entry for a key must be
   removed from the map when the stream finishes, whether it completes
   successfully, errors, or is cancelled. This must happen in the same
   `tauri::async_runtime::spawn` task that currently clears the single
   `active_cancel` slot in `commands.rs` (`send_chat_message`), so a
   finished/cancelled/failed stream never leaves a stale entry that a
   future `cancel_active_chat` call could mistakenly act on (or that would
   otherwise leak memory over a long-running session).
4. Do not change this in the same change as F1/F2 (SSE retry logic and
   message status handling) — those are Codex's responsibility per
   `audit/CLAUDE_REVIEW_NOVA_TECHNICAL_MVP.md`; a cancellation-map change
   should land as its own reviewable diff that does not touch
   `stream_chat`/`stream_chat_once` internals.

### Acceptance criteria for closing this item

- [ ] Two conversations can stream simultaneously and each has an
      independently working "停止" (stop) control that only affects its own
      stream.
- [ ] Stopping conversation A never affects conversation B's in-flight
      stream.
- [ ] After a stream finishes (complete, error, or user-cancelled), its
      entry is removed from the cancellation map — verified by a test that
      sends N sequential messages and asserts the map's size returns to 0
      after each one (no leak).
- [ ] `cancel_active_chat` (or its replacement command) fails safely (no
      panic, no-op) when called with a key that no longer has an active
      stream (e.g. user double-clicks stop, or clicks stop just as the
      stream naturally completed).
- [ ] Existing single-stream behavior (current MVP UX) continues to pass
      all existing tests unchanged.
