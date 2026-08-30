# Nova Data Model

SQLite database location is the Tauri application data directory, file `nova.sqlite3`.

## Tables

`providers`

- `id`
- `name`
- `base_url`
- `masked_key`
- `has_key`
- `updated_at`

`app_settings`

- `key`
- `value`
- `updated_at`

`conversations`

- `id`
- `title`
- `provider_id`
- `model_id`
- `created_at`
- `updated_at`

`messages`

- `id`
- `conversation_id`
- `role`
- `content`
- `provider_id`
- `model_id`
- `status`
- `request_id`
- `created_at`

`request_logs`

- `id`
- `provider_id`
- `model_id`
- `endpoint`
- `status_code`
- `error_code`
- `message`
- `request_id`
- `created_at`

## Migration Rule

Migrations are repeatable and use `CREATE TABLE IF NOT EXISTS`. API keys are explicitly absent from all tables.
