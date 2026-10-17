//! Second re-review remediation (Blocker 3): persisted, explicit user
//! confirmation that a task currently has no applicable automated tests.
//! Written exclusively by `commands::confirm_no_tests_applicable` (the only
//! caller anywhere in this codebase — never from `core_loop`, never from
//! any AI-driven path), which hardcodes `Actor::User`'s storage-layer
//! string form so the frontend cannot forge this as a system decision no
//! matter what it sends.
//!
//! Scope is deliberately narrow: a confirmation is only ever valid for the
//! *exact* `(task_id, result_generation, round)` it was recorded against
//! (enforced by `UNIQUE(task_id, result_generation, round)` plus the
//! lookup below never widening that key) — a new fix-pass round, or a new
//! result generation after acceptance, must be re-confirmed from scratch.

use rusqlite::{params, OptionalExtension};

use crate::db::Database;
use crate::error::NovaResult;
use crate::models::NoTestsConfirmation;
use crate::security::redact_sensitive_text;

use super::{new_id, now};

impl Database {
    /// Audit history only: includes NULL and superseded attempts, never authorizes work.
    pub fn list_no_tests_confirmations_for_round(&self, project_id: &str, task_id: &str, generation: i64, round: i64) -> NovaResult<Vec<NoTestsConfirmation>> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let mut stmt = self.conn().prepare("SELECT id, project_id, task_id, result_generation, round, result_attempt, confirmed_by, reason, confirmed_at, created_at FROM no_tests_confirmations WHERE project_id=?1 AND task_id=?2 AND result_generation=?3 AND round=?4 ORDER BY rowid ASC")?;
        let rows = stmt.query_map(params![project_id, task_id, generation, round], read_confirmation)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Work Package 3 final review remediation (Major 1): idempotent at the
    /// SQL level, not just the caller's — `ON CONFLICT(task_id,
    /// result_generation, round) DO NOTHING` means a second write attempt
    /// for the same natural key (a genuine duplicate request, or two
    /// concurrent requests racing each other) can never surface a `UNIQUE`
    /// constraint error and can never create a second row. The row is
    /// always re-read by the natural key afterward — never by the
    /// freshly-generated `id` — so both the request that actually inserted
    /// and any request that lost the race to an existing row return the
    /// exact same, real, persisted confirmation (an existing row's
    /// `reason` is never overwritten). A structural mirror of
    /// `Database::record_no_build_confirmation`'s identical fix.
    pub fn record_no_tests_confirmation(
        &self,
        project_id: &str,
        task_id: &str,
        result_generation: i64,
        round: i64,
        result_attempt: i64,
        reason: Option<&str>,
    ) -> NovaResult<NoTestsConfirmation> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let id = new_id();
        let now = now();
        let reason = reason.map(redact_sensitive_text);
        self.conn().execute(
            "INSERT INTO no_tests_confirmations (id, project_id, task_id, result_generation, round, result_attempt, confirmed_by, reason, confirmed_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'user', ?7, ?8, ?8)
             ON CONFLICT(task_id, result_generation, round, result_attempt) DO NOTHING",
            params![id, project_id, task_id, result_generation, round, result_attempt, reason, now],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, round, result_attempt, confirmed_by, reason, confirmed_at, created_at
                 FROM no_tests_confirmations WHERE task_id = ?1 AND result_generation = ?2 AND round = ?3 AND result_attempt = ?4",
                params![task_id, result_generation, round, result_attempt],
                read_confirmation,
            )
            .map_err(Into::into)
    }

    /// Looks up a confirmation for the *exact* current
    /// `(task_id, result_generation, round, result_attempt)` — never a
    /// prior generation, round, or dev/fix execution attempt, and never a
    /// different task, even within the same project.
    pub fn find_no_tests_confirmation(
        &self,
        project_id: &str,
        task_id: &str,
        result_generation: i64,
        round: i64,
        result_attempt: i64,
    ) -> NovaResult<Option<NoTestsConfirmation>> {
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, round, result_attempt, confirmed_by, reason, confirmed_at, created_at
                 FROM no_tests_confirmations
                 WHERE project_id = ?1 AND task_id = ?2 AND result_generation = ?3 AND round = ?4 AND result_attempt = ?5",
                params![project_id, task_id, result_generation, round, result_attempt],
                read_confirmation,
            )
            .optional()
            .map_err(Into::into)
    }
}

fn read_confirmation(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoTestsConfirmation> {
    Ok(NoTestsConfirmation {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        result_generation: row.get(3)?,
        round: row.get(4)?,
        result_attempt: row.get(5)?,
        confirmed_by: row.get(6)?,
        reason: row.get(7)?,
        confirmed_at: row.get(8)?,
        created_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Actor, CreateProjectRequest, CreateTaskRequest};
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-no-tests-confirmation-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db_with_task(dir: &std::path::Path) -> (Database, String, String) {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let project = db
            .create_project(&CreateProjectRequest {
                name: "P".to_string(),
                local_path: dir.to_string_lossy().into_owned(),
                source_type: "local".to_string(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap();
        let task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "T".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: None,
                },
                Actor::User,
            )
            .unwrap();
        (db, project.id, task.id)
    }

    #[test]
    fn records_and_finds_a_confirmation_for_the_exact_generation_and_round() {
        let dir = temp_dir("record-find");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, 1, Some("no framework yet"))
            .unwrap();
        let found = db
            .find_no_tests_confirmation(&project_id, &task_id, 0, 1, 1)
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().confirmed_by, "user");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmation_does_not_apply_to_a_different_round() {
        let dir = temp_dir("wrong-round");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, 1, None)
            .unwrap();
        let found = db
            .find_no_tests_confirmation(&project_id, &task_id, 0, 2, 1)
            .unwrap();
        assert!(
            found.is_none(),
            "a round-1 confirmation must not silently cover round 2"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmation_does_not_apply_to_a_different_generation() {
        let dir = temp_dir("wrong-generation");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, 1, None)
            .unwrap();
        let found = db
            .find_no_tests_confirmation(&project_id, &task_id, 1, 1, 1)
            .unwrap();
        assert!(
            found.is_none(),
            "a generation-0 confirmation must not silently cover generation 1 (post-fix-pass reuse)"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmation_is_scoped_to_its_own_task_even_within_the_same_project() {
        let dir = temp_dir("wrong-task");
        let (db, project_id, task_id_a) = db_with_task(&dir);
        let task_b = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project_id.clone(),
                    title: "T2".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: None,
                },
                Actor::User,
            )
            .unwrap();
        db.record_no_tests_confirmation(&project_id, &task_id_a, 0, 1, 1, None)
            .unwrap();
        let found = db
            .find_no_tests_confirmation(&project_id, &task_b.id, 0, 1, 1)
            .unwrap();
        assert!(found.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reason_is_redacted_before_storage() {
        let dir = temp_dir("redact-reason");
        let (db, project_id, task_id) = db_with_task(&dir);
        let confirmation = db
            .record_no_tests_confirmation(
                &project_id,
                &task_id,
                0,
                1,
                1,
                Some("token=abc123supersecretvalue"),
            )
            .unwrap();
        assert!(!confirmation
            .reason
            .unwrap()
            .contains("abc123supersecretvalue"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_id_that_does_not_own_the_task_is_rejected() {
        let dir_a = temp_dir("write-wrong-project-a");
        let dir_b = temp_dir("write-wrong-project-b");
        let (db, _project_a, task_a) = db_with_task(&dir_a);
        let project_b = db
            .create_project(&CreateProjectRequest {
                name: "B".to_string(),
                local_path: dir_b.to_string_lossy().into_owned(),
                source_type: "local".to_string(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap();
        let result = db.record_no_tests_confirmation(&project_b.id, &task_a, 0, 1, 1, None);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    /// WP2A B3: a confirmation recorded for one dev/fix execution attempt
    /// must not authorize a later, distinct attempt of the same
    /// `(task_id, result_generation, round)` — and the later attempt must
    /// still be able to record its own, independent confirmation (the
    /// extended `UNIQUE(task_id, result_generation, round, result_attempt)`
    /// key must not block it).
    #[test]
    fn a_new_attempt_can_independently_reconfirm_without_being_blocked_by_an_older_attempts_row() {
        let dir = temp_dir("reauthorize-new-attempt");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, 1, Some("attempt 1"))
            .unwrap();
        // The old attempt's confirmation must not satisfy a lookup for the new attempt:
        assert!(db
            .find_no_tests_confirmation(&project_id, &task_id, 0, 1, 2)
            .unwrap()
            .is_none());
        // The new attempt can record its own confirmation independently:
        let second = db
            .record_no_tests_confirmation(&project_id, &task_id, 0, 1, 2, Some("attempt 2"))
            .unwrap();
        assert_eq!(second.result_attempt, Some(2));
        // Both rows coexist — the old attempt's row is untouched, not overwritten:
        let first = db
            .find_no_tests_confirmation(&project_id, &task_id, 0, 1, 1)
            .unwrap()
            .expect("attempt 1's confirmation must still be readable");
        assert_eq!(first.reason.unwrap(), "attempt 1");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Historical rows backfilled with `result_attempt = NULL` (schema v11
    /// migration) must never satisfy a lookup for any real attempt number —
    /// NULL is never treated as a wildcard match.
    #[test]
    fn a_historical_null_result_attempt_row_never_satisfies_a_real_attempt_lookup() {
        let dir = temp_dir("historical-null-attempt");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.conn()
            .execute(
                "INSERT INTO no_tests_confirmations (id, project_id, task_id, result_generation, round, result_attempt, confirmed_by, reason, confirmed_at, created_at)
                 VALUES ('legacy-row', ?1, ?2, 0, 1, NULL, 'user', 'pre-v11 row', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                rusqlite::params![project_id, task_id],
            )
            .unwrap();
        let found = db
            .find_no_tests_confirmation(&project_id, &task_id, 0, 1, 1)
            .unwrap();
        assert!(
            found.is_none(),
            "a NULL-attempt historical row must never satisfy a lookup for a real attempt number"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
