//! Second re-review remediation (Blocker 2 / executable trust boundary):
//! persisted record of the exact CLI binary fingerprint (see
//! `runtime::binary_trust`) the user has explicitly confirmed for a given
//! `kind` (`codex_cli`/`claude_cli`). Written exclusively by
//! `commands::confirm_cli_binary_path`, which hardcodes `Actor::User`'s
//! storage-layer string — the frontend cannot forge a trust record as
//! system-originated.
//!
//! `UNIQUE(kind)` means at most one trusted binary exists per kind at a
//! time; confirming a new path replaces the old record (delete-then-insert
//! in one transaction below), so trust history is not retained — only "the
//! single path currently trusted for this kind" needs to be re-derivable
//! and re-checked before every launch.

use rusqlite::{params, OptionalExtension};

use crate::db::Database;
use crate::error::NovaResult;
use crate::models::TrustedCliBinary;
use crate::runtime::binary_trust::BinaryFingerprint;

use super::{new_id, now};

impl Database {
    pub fn confirm_trusted_cli_binary(
        &self,
        kind: &str,
        fingerprint: &BinaryFingerprint,
    ) -> NovaResult<TrustedCliBinary> {
        let id = new_id();
        let now_value = now();
        let conn = self.conn();
        conn.execute(
            "DELETE FROM trusted_cli_binaries WHERE kind = ?1",
            params![kind],
        )?;
        conn.execute(
            "INSERT INTO trusted_cli_binaries (id, kind, canonical_path, file_size, modified_unix_ms, sha256_hex, confirmed_by, confirmed_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'user', ?7, ?7)",
            params![
                id,
                kind,
                fingerprint.canonical_path,
                fingerprint.file_size as i64,
                fingerprint.modified_unix_ms,
                fingerprint.sha256_hex,
                now_value,
            ],
        )?;
        conn.query_row(
            "SELECT id, kind, canonical_path, file_size, modified_unix_ms, sha256_hex, confirmed_by, confirmed_at, created_at
             FROM trusted_cli_binaries WHERE id = ?1",
            params![id],
            read_trusted_binary,
        )
        .map_err(Into::into)
    }

    pub fn find_trusted_cli_binary(&self, kind: &str) -> NovaResult<Option<TrustedCliBinary>> {
        self.conn()
            .query_row(
                "SELECT id, kind, canonical_path, file_size, modified_unix_ms, sha256_hex, confirmed_by, confirmed_at, created_at
                 FROM trusted_cli_binaries WHERE kind = ?1",
                params![kind],
                read_trusted_binary,
            )
            .optional()
            .map_err(Into::into)
    }
}

fn read_trusted_binary(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrustedCliBinary> {
    Ok(TrustedCliBinary {
        id: row.get(0)?,
        kind: row.get(1)?,
        canonical_path: row.get(2)?,
        file_size: row.get(3)?,
        modified_unix_ms: row.get(4)?,
        sha256_hex: row.get(5)?,
        confirmed_by: row.get(6)?,
        confirmed_at: row.get(7)?,
        created_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(path: &str, size: u64, mtime: i64, hash: &str) -> BinaryFingerprint {
        BinaryFingerprint {
            canonical_path: path.to_string(),
            file_size: size,
            modified_unix_ms: mtime,
            sha256_hex: hash.to_string(),
        }
    }

    #[test]
    fn confirms_and_finds_a_trusted_binary() {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        db.confirm_trusted_cli_binary("codex_cli", &fp("C:\\codex.exe", 100, 1, "aa"))
            .unwrap();
        let found = db.find_trusted_cli_binary("codex_cli").unwrap().unwrap();
        assert_eq!(found.canonical_path, "C:\\codex.exe");
        assert_eq!(found.confirmed_by, "user");
    }

    #[test]
    fn re_confirming_replaces_the_previous_trust_record_for_the_same_kind() {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        db.confirm_trusted_cli_binary("codex_cli", &fp("C:\\old.exe", 100, 1, "aa"))
            .unwrap();
        db.confirm_trusted_cli_binary("codex_cli", &fp("C:\\new.exe", 200, 2, "bb"))
            .unwrap();
        let found = db.find_trusted_cli_binary("codex_cli").unwrap().unwrap();
        assert_eq!(found.canonical_path, "C:\\new.exe");

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM trusted_cli_binaries WHERE kind = 'codex_cli'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "only the latest confirmation must survive");
    }

    #[test]
    fn codex_and_claude_trust_are_independent() {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        db.confirm_trusted_cli_binary("codex_cli", &fp("C:\\codex.exe", 100, 1, "aa"))
            .unwrap();
        let claude = db.find_trusted_cli_binary("claude_cli").unwrap();
        assert!(claude.is_none());
    }

    #[test]
    fn an_unconfirmed_kind_returns_none() {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        assert!(db.find_trusted_cli_binary("codex_cli").unwrap().is_none());
    }
}
