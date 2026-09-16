//! Phase 3: independent review-round records (`task_reviews`). Written
//! exclusively by `crate::core_loop`, which is itself the only code path
//! that invokes a reviewer AI — never directly reachable from a Tauri
//! command, matching the same "frontend cannot forge backend-driven
//! records" principle Phase 2's actor-boundary fix established.

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

    pub fn list_task_reviews(&self, task_id: &str) -> NovaResult<Vec<TaskReview>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at
             FROM task_reviews WHERE task_id = ?1 ORDER BY result_generation ASC, round ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id], read_task_review)?
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

        let reviews = db.list_task_reviews(&task_id).unwrap();
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
}
