# Nova Architecture

## Layers

- Vue 3 UI: pages for Chat, Providers, and Settings only.
- Pinia state: transient UI state and invoke wrappers. It never persists API keys.
- Tauri command boundary: the only frontend/backend bridge.
- Rust SDKMAX client: constructs Authorization headers internally and enforces endpoint allowlist.
- Rust credential store: Windows Credential Manager for API Key persistence.
- SQLite: local conversations, messages, provider metadata, settings, and redacted request logs.

## Data Boundaries

Vue sends a key only once to `save_provider`. After saving, the key input is cleared. Later calls pass only `provider_id`; Rust reads the key from Credential Manager and constructs `Authorization: Bearer ...` internally.

## Network Boundaries

Allowed production host: `https://api.sdkmax.com`.

Allowed development host: explicit localhost only in debug builds.

Allowed paths:

- `/v1/models`
- `/v1/chat/completions`
- `/api/usage/token/`

All other paths are rejected in Rust before any request is made.
