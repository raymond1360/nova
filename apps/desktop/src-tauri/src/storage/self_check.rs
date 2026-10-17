//! WP2A: the developer AI's structured self-assessment of one round's work
//! (see `models::SelfCheck`). Append-only — a row is never updated or
//! deleted. "The current judgment" for a given `(task_id, round,
//! result_attempt)` is always the most recently inserted row for that
//! exact key, found via `ORDER BY rowid DESC LIMIT 1` (SQLite's implicit
//! row id, monotonically increasing with insertion order — this table's
//! `id TEXT PRIMARY KEY` does not alias `rowid`, so the column exists and
//! is usable without a schema change). This is deliberately a third,
//! distinct write/read semantic from both `no_tests_confirmations`/
//! `no_build_confirmations` ("first write wins", `ON CONFLICT DO NOTHING`)
//! and `test_evidence`/`build_evidence` ("any historical row with a
//! passing status counts") — see the WP2A design docs for why neither of
//! those two existing patterns is safe to reuse here: a later, corrected
//! re-parse must be able to supersede an earlier wrong one, which neither
//! "first write wins" nor "any row ever passing" would allow.

use rusqlite::params;

use crate::db::Database;
use crate::error::NovaResult;
use crate::models::SelfCheck;
use crate::security::redact_sensitive_text;

use super::{new_id, now};

/// One parsed self-check ready to persist. Parsing itself (extracting this
/// from a `DEVELOPMENT_REPORT.md`/`FIX_REPORT.md` fenced block) is the
/// caller's responsibility — this module only stores/retrieves already-
/// parsed results, same separation of concerns as every other evidence
/// table in this codebase.
pub struct RecordSelfCheckRequest<'a> {
    pub project_id: &'a str,
    pub task_id: &'a str,
    pub round: i64,
    pub result_attempt: i64,
    pub parse_status: &'a str,
    pub self_reported_status: Option<&'a str>,
    pub requirement_coverage_json: Option<&'a str>,
    pub check_results_json: Option<&'a str>,
    pub unfinished_items_json: Option<&'a str>,
    pub blocking_issues_json: Option<&'a str>,
    pub raw_block_redacted: &'a str,
}

impl Database {
    pub fn record_self_check(&self, request: &RecordSelfCheckRequest<'_>) -> NovaResult<SelfCheck> {
        self.assert_task_belongs_to_project(request.project_id, request.task_id)?;
        let id = new_id();
        let now = now();
        let raw_block_redacted = redact_sensitive_text(request.raw_block_redacted);
        self.conn().execute(
            "INSERT INTO self_checks (id, project_id, task_id, round, result_attempt, parse_status, self_reported_status, requirement_coverage_json, check_results_json, unfinished_items_json, blocking_issues_json, raw_block_redacted, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                request.project_id,
                request.task_id,
                request.round,
                request.result_attempt,
                request.parse_status,
                request.self_reported_status,
                request.requirement_coverage_json,
                request.check_results_json,
                request.unfinished_items_json,
                request.blocking_issues_json,
                raw_block_redacted,
                now,
            ],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, round, result_attempt, parse_status, self_reported_status, requirement_coverage_json, check_results_json, unfinished_items_json, blocking_issues_json, raw_block_redacted, created_at
                 FROM self_checks WHERE id = ?1",
                params![id],
                read_self_check,
            )
            .map_err(Into::into)
    }

    /// The authoritative "current self-check" for one exact
    /// `(task_id, round, result_attempt)` — the most recently inserted row
    /// for that key, or `None` if no self-check has ever been recorded for
    /// it (including the case where the key's `result_attempt` has since
    /// moved on — an old attempt's rows are simply never matched by a
    /// lookup for the new attempt's number).
    pub fn current_self_check(
        &self,
        task_id: &str,
        round: i64,
        result_attempt: i64,
    ) -> NovaResult<Option<SelfCheck>> {
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, round, result_attempt, parse_status, self_reported_status, requirement_coverage_json, check_results_json, unfinished_items_json, blocking_issues_json, raw_block_redacted, created_at
                 FROM self_checks
                 WHERE task_id = ?1 AND round = ?2 AND result_attempt = ?3
                 ORDER BY rowid DESC LIMIT 1",
                params![task_id, round, result_attempt],
                read_self_check,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Every self-check row ever recorded for a task, oldest first — for
    /// audit/technical-detail display only, never for gate decisions (use
    /// [`Database::current_self_check`] for that).
    pub fn list_self_checks_for_task(&self, task_id: &str) -> NovaResult<Vec<SelfCheck>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, round, result_attempt, parse_status, self_reported_status, requirement_coverage_json, check_results_json, unfinished_items_json, blocking_issues_json, raw_block_redacted, created_at
             FROM self_checks WHERE task_id = ?1 ORDER BY rowid ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id], read_self_check)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

use rusqlite::OptionalExtension;

fn read_self_check(row: &rusqlite::Row<'_>) -> rusqlite::Result<SelfCheck> {
    Ok(SelfCheck {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        round: row.get(3)?,
        result_attempt: row.get(4)?,
        parse_status: row.get(5)?,
        self_reported_status: row.get(6)?,
        requirement_coverage_json: row.get(7)?,
        check_results_json: row.get(8)?,
        unfinished_items_json: row.get(9)?,
        blocking_issues_json: row.get(10)?,
        raw_block_redacted: row.get(11)?,
        created_at: row.get(12)?,
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
            "nova-self-check-test-{name}-{}-{}",
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
                    goal: Some("goal".to_string()),
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

    fn valid_request<'a>(project_id: &'a str, task_id: &'a str, round: i64, attempt: i64) -> RecordSelfCheckRequest<'a> {
        RecordSelfCheckRequest {
            project_id,
            task_id,
            round,
            result_attempt: attempt,
            parse_status: "valid",
            self_reported_status: Some("self_reported_complete"),
            requirement_coverage_json: Some(r#"[{"item":"goal:0","status":"covered"}]"#),
            check_results_json: Some("[]"),
            unfinished_items_json: Some("[]"),
            blocking_issues_json: Some("[]"),
            raw_block_redacted: "raw block",
        }
    }

    #[test]
    fn records_and_finds_the_current_self_check_for_an_exact_key() {
        let dir = temp_dir("record-find");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_self_check(&valid_request(&project_id, &task_id, 1, 1))
            .unwrap();
        let found = db.current_self_check(&task_id, 1, 1).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().parse_status, "valid");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_later_row_supersedes_an_earlier_one_for_the_same_key() {
        let dir = temp_dir("supersede");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_self_check(&valid_request(&project_id, &task_id, 1, 1))
            .unwrap();
        // A corrected re-parse later determines the block was actually invalid:
        let mut invalid = valid_request(&project_id, &task_id, 1, 1);
        invalid.parse_status = "invalid_format";
        invalid.self_reported_status = None;
        invalid.requirement_coverage_json = None;
        invalid.check_results_json = None;
        invalid.unfinished_items_json = None;
        invalid.blocking_issues_json = None;
        db.record_self_check(&invalid).unwrap();
        let current = db.current_self_check(&task_id, 1, 1).unwrap().unwrap();
        assert_eq!(
            current.parse_status, "invalid_format",
            "the later, corrected row must supersede the earlier valid one"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lookup_for_a_different_attempt_never_finds_an_older_attempts_row() {
        let dir = temp_dir("wrong-attempt");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_self_check(&valid_request(&project_id, &task_id, 1, 1))
            .unwrap();
        let found = db.current_self_check(&task_id, 1, 2).unwrap();
        assert!(
            found.is_none(),
            "attempt 1's self-check must not satisfy a lookup for attempt 2"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lookup_for_a_different_round_never_finds_another_rounds_row() {
        let dir = temp_dir("wrong-round");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_self_check(&valid_request(&project_id, &task_id, 1, 1))
            .unwrap();
        let found = db.current_self_check(&task_id, 2, 1).unwrap();
        assert!(found.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn raw_block_is_redacted_before_storage() {
        let dir = temp_dir("redact");
        let (db, project_id, task_id) = db_with_task(&dir);
        let mut request = valid_request(&project_id, &task_id, 1, 1);
        request.raw_block_redacted = "token=abc123supersecretvalue";
        let recorded = db.record_self_check(&request).unwrap();
        assert!(!recorded.raw_block_redacted.contains("abc123supersecretvalue"));
        let _ = fs::remove_dir_all(&dir);
    }
}
