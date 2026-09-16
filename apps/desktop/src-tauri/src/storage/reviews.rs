//! Phase 3: independent review-round records (`task_reviews`). Written
//! exclusively by `crate::core_loop`, which is itself the only code path
//! that invokes a reviewer AI — never directly reachable from a Tauri
//! command, matching the same "frontend cannot forge backend-driven
//! records" principle Phase 2's actor-boundary fix established.
//!
//! Phase 3 remediation (Major 3): schema v5's composite foreign key
//! `(task_id, project_id) REFERENCES tasks(id, project_id)` already makes an
//! inconsistent `project_id` impossible to *write* at the SQLite level, but
//! a raw FK violation is an unfriendly, hard-to-classify `rusqlite::Error`
//! for a caller to handle. Both methods here re-verify ownership themselves
//! first, reusing `storage::evidence`'s `assert_task_belongs_to_project`
//! guard, so a mismatch fails fast with a clear `NovaError::Config` — the FK
//! stays as a second, defense-in-depth layer, never the only one.
//! `list_task_reviews` additionally now requires the caller's `project_id`,
//! so the frontend can no longer read another project's review findings
//! just by knowing/guessing a `task_id` (Codex review Major 3, "跨项目越权").

use rusqlite::params;

use crate::db::Database;
use crate::error::NovaResult;
use crate::models::TaskReview;
use crate::security::redact_sensitive_text;

use super::{new_id, now};

impl Database {
    #[allow(clippy::too_many_arguments)]
    pub fn record_task_review(
        &self,
        project_id: &str,
        task_id: &str,
        result_generation: i64,
        round: i64,
        reviewer_ai_kind: &str,
        verdict: &str,
        findings_summary: Option<&str>,
    ) -> NovaResult<TaskReview> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let id = new_id();
        let now = now();
        let findings_summary = findings_summary.map(redact_sensitive_text);
        self.conn().execute(
            "INSERT INTO task_reviews (id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, now],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at
                 FROM task_reviews WHERE id = ?1",
                params![id],
                read_task_review,
            )
            .map_err(Into::into)
    }

    /// `project_id` is required (not just `task_id`) so the caller's
    /// project-scoped session can never read a different project's review
    /// findings by supplying a `task_id` it does not actually own.
    pub fn list_task_reviews(
        &self,
        project_id: &str,
        task_id: &str,
    ) -> NovaResult<Vec<TaskReview>> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at
             FROM task_reviews WHERE task_id = ?1 AND project_id = ?2 ORDER BY result_generation ASC, round ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id, project_id], read_task_review)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn read_task_review(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskReview> {
    Ok(TaskReview {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        result_generation: row.get(3)?,
        round: row.get(4)?,
        reviewer_ai_kind: row.get(5)?,
        verdict: row.get(6)?,
        findings_summary: row.get(7)?,
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
            "nova-reviews-test-{name}-{}-{}",
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
    fn records_and_lists_review_rounds_in_order() {
        let dir = temp_dir("record-review");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_task_review(
            &project_id,
            &task_id,
            1,
            1,
            "claude_cli",
            "changes_required",
            Some("needs work"),
        )
        .unwrap();
        db.record_task_review(
            &project_id,
            &task_id,
            1,
            2,
            "claude_cli",
            "approve",
            Some("looks good"),
        )
        .unwrap();

        let reviews = db.list_task_reviews(&project_id, &task_id).unwrap();
        assert_eq!(reviews.len(), 2);
        assert_eq!(reviews[0].round, 1);
        assert_eq!(reviews[1].round, 2);
        assert_eq!(reviews[1].verdict, "approve");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn findings_summary_is_redacted_before_storage() {
        let dir = temp_dir("redact-findings");
        let (db, project_id, task_id) = db_with_task(&dir);
        let review = db
            .record_task_review(
                &project_id,
                &task_id,
                1,
                1,
                "codex_cli",
                "changes_required",
                Some("token=abc123supersecretvalue"),
            )
            .unwrap();
        let findings = review.findings_summary.unwrap();
        assert!(!findings.contains("abc123supersecretvalue"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_round_for_the_same_generation_is_rejected() {
        let dir = temp_dir("duplicate-round");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_task_review(&project_id, &task_id, 1, 1, "claude_cli", "approve", None)
            .unwrap();
        let result =
            db.record_task_review(&project_id, &task_id, 1, 1, "claude_cli", "approve", None);
        assert!(
            result.is_err(),
            "UNIQUE(task_id, result_generation, round) must reject a duplicate round"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_verdict_is_rejected_by_the_check_constraint() {
        let dir = temp_dir("bad-verdict");
        let (db, project_id, task_id) = db_with_task(&dir);
        let result =
            db.record_task_review(&project_id, &task_id, 1, 1, "claude_cli", "maybe", None);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Major 3: task_reviews project ownership ---------------------------

    #[test]
    fn writing_a_review_with_a_project_id_that_does_not_own_the_task_is_rejected() {
        let dir_a = temp_dir("write-wrong-project-a");
        let dir_b = temp_dir("write-wrong-project-b");
        let (db, _project_a, task_a) = db_with_task(&dir_a);
        let db2 = Database::memory().unwrap();
        // Independent second project in the *same* database, so its id is
        // guaranteed to exist but genuinely not own `task_a`.
        drop(db2);
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

        let result =
            db.record_task_review(&project_b.id, &task_a, 1, 1, "claude_cli", "approve", None);
        assert!(
            result.is_err(),
            "a project_id that does not own task_a must be rejected before any write"
        );
        let reviews = db.list_task_reviews(&_project_a, &task_a).unwrap();
        assert!(
            reviews.is_empty(),
            "the rejected write must not have left a row behind"
        );
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn listing_reviews_with_a_project_id_that_does_not_own_the_task_is_rejected() {
        let dir_a = temp_dir("list-wrong-project-a");
        let dir_b = temp_dir("list-wrong-project-b");
        let (db, project_a, task_a) = db_with_task(&dir_a);
        db.record_task_review(&project_a, &task_a, 1, 1, "claude_cli", "approve", None)
            .unwrap();
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

        let result = db.list_task_reviews(&project_b.id, &task_a);
        assert!(
            result.is_err(),
            "the frontend must not be able to read task_a's reviews by supplying project_b's id, \
             even though task_a genuinely has a review row"
        );
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }
}
