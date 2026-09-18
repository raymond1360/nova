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
    pub fn record_no_tests_confirmation(
        &self,
        project_id: &str,
        task_id: &str,
        result_generation: i64,
        round: i64,
        reason: Option<&str>,
    ) -> NovaResult<NoTestsConfirmation> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let id = new_id();
        let now = now();
        let reason = reason.map(redact_sensitive_text);
        self.conn().execute(
            "INSERT INTO no_tests_confirmations (id, project_id, task_id, result_generation, round, confirmed_by, reason, confirmed_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'user', ?6, ?7, ?7)",
            params![id, project_id, task_id, result_generation, round, reason, now],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, round, confirmed_by, reason, confirmed_at, created_at
                 FROM no_tests_confirmations WHERE id = ?1",
                params![id],
                read_confirmation,
            )
            .map_err(Into::into)
    }

    /// Looks up a confirmation for the *exact* current
    /// `(task_id, result_generation, round)` — never a prior generation or
    /// round, and never a different task, even within the same project.
    pub fn find_no_tests_confirmation(
        &self,
        project_id: &str,
        task_id: &str,
        result_generation: i64,
        round: i64,
    ) -> NovaResult<Option<NoTestsConfirmation>> {
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, round, confirmed_by, reason, confirmed_at, created_at
                 FROM no_tests_confirmations
                 WHERE project_id = ?1 AND task_id = ?2 AND result_generation = ?3 AND round = ?4",
                params![project_id, task_id, result_generation, round],
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
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, Some("no framework yet"))
            .unwrap();
        let found = db
            .find_no_tests_confirmation(&project_id, &task_id, 0, 1)
            .unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().confirmed_by, "user");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmation_does_not_apply_to_a_different_round() {
        let dir = temp_dir("wrong-round");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let found = db
            .find_no_tests_confirmation(&project_id, &task_id, 0, 2)
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
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let found = db
            .find_no_tests_confirmation(&project_id, &task_id, 1, 1)
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
        db.record_no_tests_confirmation(&project_id, &task_id_a, 0, 1, None)
            .unwrap();
        let found = db
            .find_no_tests_confirmation(&project_id, &task_b.id, 0, 1)
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
        let result = db.record_no_tests_confirmation(&project_b.id, &task_a, 0, 1, None);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }
}
