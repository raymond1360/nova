# Nova Security Spec

## API Key Handling

- API keys are accepted only by `save_provider`.
- API keys are stored in Windows Credential Manager.
- API keys are not stored in SQLite, localStorage, sessionStorage, frontend source, request logs, or app settings.
- Provider metadata stores only `masked_key` and `has_key`.
- Deleting a provider credential deletes the system credential and clears local metadata.

## Logging

Request logs may contain endpoint path, status code, error code, redacted message, model id, provider id, and masked request id. They must not contain request bodies, Authorization headers, Cookie values, or full secrets.

## Forbidden SDKMAX Surfaces

Nova 0.1 must not call:

- `/api/channel/*`
- `/api/option/*`
- `/api/models/*` admin APIs
- `/api/ratio_sync/*`
- `/api/openrouter_sync/*`
- `/api/user/manage*`
- admin `/api/redemption/*`
- payment webhooks
- direct DB or Redis

## WebView Content Security Policy

`apps/desktop/src-tauri/tauri.conf.json` sets `app.security.csp` to a minimal policy for production builds: `default-src 'self'` with no `unsafe-inline` or `unsafe-eval` for scripts, `connect-src` limited to `'self'` plus Tauri's own IPC origin (`ipc: http://ipc.localhost`), and `object-src`/`base-uri`/`form-action`/`frame-ancestors` all disabled. The frontend does not open `connect-src` directly to `https://api.sdkmax.com`; all SDKMAX network calls are made by the Rust side, matching the data boundary in `NOVA_ARCHITECTURE.md`.

A separate, slightly relaxed `app.security.devCsp` applies only to `npm run tauri dev` (external Vite dev server), adding `'unsafe-inline'` to `style-src` (required for Vite's dev-time CSS injection) and the Vite HMR websocket origin to `connect-src`. This relaxation never ships in a production build — `tauri build` uses `csp`, not `devCsp`.

## Current Limitations

Nova 0.1 does not include a Nova-specific OAuth/device authorization service. Until SDKMAX adds a desktop authorization flow, users must manually provide an SDKMAX API Key.

Known accepted technical debt (not a security gap in the current single-stream MVP UI, but tracked for when the product surface grows) is recorded in `NOVA_TECHNICAL_DEBT.md`.
