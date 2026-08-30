use std::path::Path;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::models::{ChatMessage, Conversation, ProviderSummary};
use crate::security::mask_secret;

pub struct Database {
    conn: Connection,
}

pub struct RequestLogEntry<'a> {
    pub provider_id: &'a str,
    pub model_id: Option<&'a str>,
    pub endpoint: &'a str,
    pub status_code: Option<u16>,
    pub error_code: Option<&'a str>,
    pub message: Option<&'a str>,
    pub request_id: Option<&'a str>,
}

impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Ok(Self {
            conn: Connection::open(path)?,
        })
    }

    #[cfg(test)]
    pub fn memory() -> rusqlite::Result<Self> {
        Ok(Self {
            conn: Connection::open_in_memory()?,
        })
    }

    pub fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS providers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                base_url TEXT NOT NULL,
                masked_key TEXT,
                has_key INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS conversations (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                model_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                model_id TEXT NOT NULL,
                status TEXT NOT NULL,
                request_id TEXT,
                created_at TEXT NOT NULL,
                FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS request_logs (
                id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL,
                model_id TEXT,
                endpoint TEXT NOT NULL,
                status_code INTEGER,
                error_code TEXT,
                message TEXT,
                request_id TEXT,
                created_at TEXT NOT NULL
            );
            ",
        )
    }

    pub fn ensure_default_provider(&self) -> rusqlite::Result<()> {
        let now = now();
        self.conn.execute(
            "
            INSERT INTO providers (id, name, base_url, masked_key, has_key, updated_at)
            VALUES ('sdkmax', 'SDKMAX', 'https://api.sdkmax.com', NULL, 0, ?1)
            ON CONFLICT(id) DO NOTHING
            ",
            params![now],
        )?;
        Ok(())
    }

    pub fn provider(&self, provider_id: &str) -> rusqlite::Result<ProviderSummary> {
        self.conn.query_row(
            "SELECT id, name, base_url, masked_key, has_key, updated_at FROM providers WHERE id = ?1",
            params![provider_id],
            |row| {
                Ok(ProviderSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    base_url: row.get(2)?,
                    masked_key: row.get(3)?,
                    has_key: row.get::<_, i64>(4)? == 1,
                    updated_at: row.get(5)?,
                })
            },
        )
    }

    pub fn save_provider_metadata(
        &self,
        provider_id: &str,
        name: &str,
        base_url: &str,
        api_key: Option<&str>,
    ) -> rusqlite::Result<ProviderSummary> {
        let now = now();
        let masked = api_key.map(mask_secret);
        self.conn.execute(
            "
            INSERT INTO providers (id, name, base_url, masked_key, has_key, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
              name = excluded.name,
              base_url = excluded.base_url,
              masked_key = COALESCE(excluded.masked_key, providers.masked_key),
              has_key = CASE WHEN excluded.has_key = 1 THEN 1 ELSE providers.has_key END,
              updated_at = excluded.updated_at
            ",
            params![
                provider_id,
                name,
                base_url,
                masked,
                api_key.is_some() as i64,
                now
            ],
        )?;
        self.provider(provider_id)
    }

    pub fn clear_provider_key(&self, provider_id: &str) -> rusqlite::Result<ProviderSummary> {
        self.conn.execute(
            "UPDATE providers SET masked_key = NULL, has_key = 0, updated_at = ?1 WHERE id = ?2",
            params![now(), provider_id],
        )?;
        self.provider(provider_id)
    }

    pub fn get_setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "
            INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at
            ",
            params![key, value, now()],
        )?;
        Ok(())
    }

    pub fn list_conversations(&self) -> rusqlite::Result<Vec<Conversation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, provider_id, model_id, created_at, updated_at FROM conversations ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], read_conversation)?.collect();
        rows
    }

    pub fn create_conversation(
        &self,
        provider_id: &str,
        model_id: Option<&str>,
    ) -> rusqlite::Result<Conversation> {
        let id = Uuid::new_v4().to_string();
        let now = now();
        self.conn.execute(
            "INSERT INTO conversations (id, title, provider_id, model_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, "新会话", provider_id, model_id, now, now],
        )?;
        self.conversation(&id)
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM conversations WHERE id = ?1",
            params![conversation_id],
        )?;
        Ok(())
    }

    pub fn conversation(&self, conversation_id: &str) -> rusqlite::Result<Conversation> {
        self.conn.query_row(
            "SELECT id, title, provider_id, model_id, created_at, updated_at FROM conversations WHERE id = ?1",
            params![conversation_id],
            read_conversation,
        )
    }

    pub fn list_messages(&self, conversation_id: &str) -> rusqlite::Result<Vec<ChatMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, conversation_id, role, content, provider_id, model_id, status, request_id, created_at FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![conversation_id], read_message)?
            .collect();
        rows
    }

    pub fn create_message(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
        provider_id: &str,
        model_id: &str,
        status: &str,
    ) -> rusqlite::Result<ChatMessage> {
        let id = Uuid::new_v4().to_string();
        let created_at = now();
        self.conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, provider_id, model_id, status, request_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)",
            params![id, conversation_id, role, content, provider_id, model_id, status, created_at],
        )?;
        self.touch_conversation(conversation_id, model_id, content)?;
        self.message(&id)
    }

    pub fn message(&self, message_id: &str) -> rusqlite::Result<ChatMessage> {
        self.conn.query_row(
            "SELECT id, conversation_id, role, content, provider_id, model_id, status, request_id, created_at FROM messages WHERE id = ?1",
            params![message_id],
            read_message,
        )
    }

    pub fn append_message_delta(
        &self,
        message_id: &str,
        delta: &str,
        request_id: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE messages SET content = content || ?1, status = 'streaming', request_id = COALESCE(?2, request_id) WHERE id = ?3",
            params![delta, request_id, message_id],
        )?;
        Ok(())
    }

    pub fn update_message_status(
        &self,
        message_id: &str,
        status: &str,
        request_id: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE messages SET status = ?1, request_id = COALESCE(?2, request_id) WHERE id = ?3",
            params![status, request_id, message_id],
        )?;
        Ok(())
    }

    pub fn log_request(&self, entry: RequestLogEntry<'_>) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO request_logs (id, provider_id, model_id, endpoint, status_code, error_code, message, request_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                Uuid::new_v4().to_string(),
                entry.provider_id,
                entry.model_id,
                entry.endpoint,
                entry.status_code.map(i64::from),
                entry.error_code,
                entry.message,
                entry.request_id,
                now()
            ],
        )?;
        Ok(())
    }

    fn touch_conversation(
        &self,
        conversation_id: &str,
        model_id: &str,
        title_seed: &str,
    ) -> rusqlite::Result<()> {
        let title = if title_seed.trim().is_empty() {
            "新会话".to_string()
        } else {
            title_seed.chars().take(24).collect::<String>()
        };
        self.conn.execute(
            "
            UPDATE conversations
            SET model_id = ?1,
                title = CASE WHEN title = '新会话' THEN ?2 ELSE title END,
                updated_at = ?3
            WHERE id = ?4
            ",
            params![model_id, title, now(), conversation_id],
        )?;
        Ok(())
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn read_conversation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get(0)?,
        title: row.get(1)?,
        provider_id: row.get(2)?,
        model_id: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn read_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        role: row.get(2)?,
        content: row.get(3)?,
        provider_id: row.get(4)?,
        model_id: row.get(5)?,
        status: row.get(6)?,
        request_id: row.get(7)?,
        created_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_repeatable_and_conversation_restores() {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        db.migrate().unwrap();
        db.ensure_default_provider().unwrap();
        let provider = db.provider("sdkmax").unwrap();
        assert!(!provider.has_key);

        let conversation = db.create_conversation("sdkmax", Some("gpt-test")).unwrap();
        db.create_message(
            &conversation.id,
            "user",
            "hello",
            "sdkmax",
            "gpt-test",
            "complete",
        )
        .unwrap();
        let restored = db.list_conversations().unwrap();
        let messages = db.list_messages(&conversation.id).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "hello");
    }

    #[test]
    fn provider_metadata_never_contains_raw_key() {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let fake_key = ["sk", "abcdefghijklmnopqrstuvwxyz"].join("-");
        let provider = db
            .save_provider_metadata(
                "sdkmax",
                "SDKMAX",
                "https://api.sdkmax.com",
                Some(&fake_key),
            )
            .unwrap();
        assert!(provider.has_key);
        assert_ne!(provider.masked_key.unwrap(), fake_key);
    }

    #[test]
    fn interrupted_partial_stream_preserves_single_partial_content() {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let conversation = db.create_conversation("sdkmax", Some("gpt-test")).unwrap();
        let assistant = db
            .create_message(
                &conversation.id,
                "assistant",
                "",
                "sdkmax",
                "gpt-test",
                "streaming",
            )
            .unwrap();

        db.append_message_delta(&assistant.id, "partial", None)
            .unwrap();
        db.update_message_status(&assistant.id, "interrupted", None)
            .unwrap();

        let messages = db.list_messages(&conversation.id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "partial");
        assert_eq!(messages[0].status, "interrupted");
    }
}
