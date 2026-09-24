//! Codex re-review remediation (Major 2): persisted, explicit user
//! confirmation that Nova could not reliably determine whether a task's
//! project needs a build step (unreadable/malformed `package.json`, or an
//! unrecognized project type) and the user has confirmed no build applies.
//! A structural mirror of `storage::no_tests_confirmation` — same shape,
//! same scoping discipline, same write/read pattern — kept as its own
//! table since it records a distinct fact ("no build needed" is not "no
//! tests needed"). Written exclusively by
//! `commands::confirm_no_build_applicable` (the only caller anywhere in
//! this codebase — never from `core_loop`, never from any AI-driven path),
//! which hardcodes `Actor::User`'s storage-layer string form so the
//! frontend cannot forge this as a system decision no matter what it
//! sends.
//!
//! Scope is deliberately narrow: a confirmation is only ever valid for the
//! *exact* `(task_id, result_generation, round)` it was recorded against
//! (enforced by `UNIQUE(task_id, result_generation, round)` plus the
//! lookup below never widening that key) — a new fix-pass round, or a new
//! result generation after acceptance, must be re-confirmed from scratch.

use rusqlite::{params, OptionalExtension};

use crate::db::Database;
use crate::error::NovaResult;
use crate::models::NoBuildConfirmation;
use crate::security::redact_sensitive_text;

use super::{new_id, now};

impl Database {
    /// Codex re-review remediation (final Major closing Work Package 1):
    /// idempotent at the SQL level, not just the caller's — `ON
    /// CONFLICT(task_id, result_generation, round) DO NOTHING` means a
    /// second write attempt for the same natural key (a genuine duplicate
    /// request, or two concurrent requests racing each other) can never
    /// surface a `UNIQUE` constraint error and can never create a second
    /// row. The row is always re-read by the natural key afterward — never
    /// by the freshly-generated `id` — so both the request that actually
    /// inserted and any request that lost the race to an existing row
    /// return the exact same, real, persisted confirmation. This is a
    /// second line of defense beneath `core_loop::confirm_no_build_
    /// applicable`'s own app-level idempotent lookup (itself sufficient
    /// under Nova's single-mutex-serialized `AppState.db`), kept in case a
    /// future refactor ever removes that serialization.
    pub fn record_no_build_confirmation(
        &self,
        project_id: &str,
        task_id: &str,
        result_generation: i64,
        round: i64,
        reason: Option<&str>,
    ) -> NovaResult<NoBuildConfirmation> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let id = new_id();
        let now = now();
        let reason = reason.map(redact_sensitive_text);
        self.conn().execute(
            "INSERT INTO no_build_confirmations (id, project_id, task_id, result_generation, round, confirmed_by, reason, confirmed_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'user', ?6, ?7, ?7)
             ON CONFLICT(task_id, result_generation, round) DO NOTHING",
            params![id, project_id, task_id, result_generation, round, reason, now],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, round, confirmed_by, reason, confirmed_at, created_at
                 FROM no_build_confirmations WHERE task_id = ?1 AND result_generation = ?2 AND round = ?3",
                params![task_id, result_generation, round],
                read_confirmation,
            )
            .map_err(Into::into)
    }

    /// Looks up a confirmation for the *exact* current
    /// `(task_id, result_generation, round)` — never a prior generation or
    /// round, and never a different task, even within the same project.
    pub fn find_no_build_confirmation(
        &self,
        project_id: &str,
        task_id: &str,
        result_generation: i64,
        round: i64,
    ) -> NovaResult<Option<NoBuildConfirmation>> {
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, round, confirmed_by, reason, confirmed_at, created_at
                 FROM no_build_confirmations
                 WHERE project_id = ?1 AND task_id = ?2 AND result_generation = ?3 AND round = ?4",
                params![project_id, task_id, result_generation, round],
                read_confirmation,
            )
            .optional()
            .map_err(Into::into)
    }
}

fn read_confirmation(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoBuildConfirmation> {
    Ok(NoBuildConfirmation {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        result_generation: row.get(3)?,
        round: row.get(4)?,
        confirmed_by: row.get(5)?,
        reason: row.get(6)?,
        confirmed_at: row.get(7)?,
        created_at: row.get(8)?,
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
            "nova-no-build-confirmation-test-{name}-{}-{}",
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
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, Some("unknown project type"))
            .unwrap();
        let found = db
            .find_no_build_confirmation(&project_id, &task_id, 0, 1)
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().confirmed_by, "user");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmation_does_not_apply_to_a_different_round() {
        let dir = temp_dir("wrong-round");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let found = db
            .find_no_build_confirmation(&project_id, &task_id, 0, 2)
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
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let found = db
            .find_no_build_confirmation(&project_id, &task_id, 1, 1)
            .unwrap();
        assert!(
            found.is_none(),
            "a generation-0 confirmation must not silently cover generation 1"
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
        db.record_no_build_confirmation(&project_id, &task_id_a, 0, 1, None)
            .unwrap();
        let found = db
            .find_no_build_confirmation(&project_id, &task_b.id, 0, 1)
            .unwrap();
        assert!(found.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reason_is_redacted_before_storage() {
        let dir = temp_dir("redact-reason");
        let (db, project_id, task_id) = db_with_task(&dir);
        let confirmation = db
            .record_no_build_confirmation(
                &project_id,
                &task_id,
                0,
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
        let result = db.record_no_build_confirmation(&project_b.id, &task_a, 0, 1, None);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }
}
