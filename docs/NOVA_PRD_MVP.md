# Nova Technical MVP 0.1 PRD

> Historical document only. This file describes the earlier SDKMAX API-key chat MVP and is not the current Nova v0.1.0-beta.1 product baseline. For current scope, use `D:\nova\product\NOVA_V1_MVP_PRODUCT_DEVELOPMENT_SPEC_V3.md` and `D:\nova\product\NOVA_V0.1.0_MINIMUM_RELEASE_DEVELOPMENT_PLAN.md`.

## Scope

Nova 0.1 is a Windows x64 desktop MVP for SDKMAX API-Key based chat usage. It does not implement SDKMAX web login, automatic `nova-desktop` token creation, payment, admin features, or SDKMAX server changes.

## User Flows

1. Configure SDKMAX provider with `https://api.sdkmax.com` and a user supplied API Key.
2. Save the key through a Tauri command into Windows Credential Manager.
3. Test connection through `/v1/models`.
4. Refresh actual callable model list from `/v1/models`.
5. Select a model and send chat messages through `/v1/chat/completions` with streaming enabled.
6. Stop an active stream and keep partial assistant output marked as interrupted.
7. Restart Nova and restore local conversations from SQLite.
8. Refresh Token usage through `/api/usage/token/`.

## Non-Goals

- No SDKMAX login/session reuse.
- No automatic Token creation or remote device revocation.
- No direct browser or Vue access to API keys.
- No admin, channel, pricing, ratio, payment webhook, redemption, user-management, DB, or Redis access.
- No production deployment or SDKMAX code modification.
