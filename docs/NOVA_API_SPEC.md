# Nova API Spec

## Tauri Commands

- `initialize_app()`
- `get_provider(providerId)`
- `save_provider({ provider_id, name, base_url, api_key })`
- `delete_provider_credential(providerId)`
- `fetch_models(providerId)`
- `test_provider_connection(providerId)`
- `fetch_token_usage(providerId)`
- `get_setting(key)`
- `set_setting(key, value)`
- `list_conversations()`
- `create_conversation(providerId, modelId)`
- `delete_conversation(conversationId)`
- `list_messages(conversationId)`
- `send_chat_message({ conversation_id, provider_id, model_id, content })`
- `cancel_active_chat()`

## SDKMAX Calls

`GET /v1/models`

Headers are created only in Rust:

```http
Authorization: Bearer [REDACTED]
```

Response shape:

```json
{
  "object": "list",
  "data": [{ "id": "model-id", "object": "model", "owned_by": "sdkmax" }]
}
```

`POST /v1/chat/completions`

```json
{
  "model": "model-id",
  "messages": [{ "role": "user", "content": "你好" }],
  "stream": true
}
```

SSE chunks use `data:` lines. Nova parses `choices[0].delta.content` and stops on `data: [DONE]`.

`GET /api/usage/token/`

Used only for display. Passing this endpoint does not prove chat calls are available; Nova combines it with `/v1/models` or a real chat request.
