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

## Current Limitations

Nova 0.1 does not include a Nova-specific OAuth/device authorization service. Until SDKMAX adds a desktop authorization flow, users must manually provide an SDKMAX API Key.
