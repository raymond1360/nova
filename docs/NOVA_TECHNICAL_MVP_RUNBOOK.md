# Nova Technical MVP Runbook

## Start

```powershell
cd D:\nova\apps\desktop
npm run tauri dev
```

## Configure Provider

1. Open Providers.
2. Keep Base URL as `https://api.sdkmax.com` unless using explicit localhost in development.
3. Enter an SDKMAX user API Key.
4. Save.
5. Test connection.
6. Refresh models.

## Chat

1. Open Chat.
2. Select a model returned by `/v1/models`.
3. Send a short message.
4. Use Stop to cancel an active stream.

## Troubleshooting

- 401: key invalid or expired.
- 403: account, token, or model permission unavailable.
- 429: rate limit or insufficient quota.
- `model_not_found`: model not callable for this key.
- `get_channel_failed`: SDKMAX has no available upstream channel for the request.
- Stream interruption: partial content is preserved locally.

## Local Artifacts

- Test logs: `D:\nova\test-results`
- Source: `D:\nova\apps\desktop`
- SQLite runtime data: Tauri app data directory, not the repository.
- Credential runtime data: Windows Credential Manager target `com.sdkmax.nova/sdkmax/api-key`.
