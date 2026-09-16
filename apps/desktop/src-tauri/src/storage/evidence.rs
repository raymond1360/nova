//! Phase 4 evidence-structure foundation: schema (see
//! `storage::migrations`), models (see `models.rs`), and the repository
//! functions below. Phase 2 only builds and tests this layer — nothing here
//! is wired to a real AI, a real command execution, or a completion gate
//! yet (that happens once Phase 3's real execution loop exists). No
//! function in this module fabricates a "passed" result: every value
//! written is exactly what the caller supplies, and callers in Phase 2 are
//! only tests.
//!
//! Nothing here is reachable from a Tauri command yet, by design — every
//! item in this module is exercised exclusively by its own `#[cfg(test)]`
//! suite until Phase 3/4 wires in real callers.
#![allow(dead_code)]

use rusqlite::params;

use crate::db::Database;
use crate::error::{NovaError, NovaResult};
use crate::models::{
    CommandEvidence, FileChangeEvidence, GeneratedDocument, GitEvidence, TestEvidence,
    TimelineEvent, UserAcceptance,
};
use crate::security::redact_sensitive_text;

use super::{new_id, now};

impl Database {
    /// Fails if `task_id` does not belong to `project_id` — the guard every
    /// evidence-writing function below runs first, so evidence can never be
    /// attributed to the wrong project even if a caller passes a mismatched
    /// pair by mistake.
    fn assert_task_belongs_to_project(&self, project_id: &str, task_id: &str) -> NovaResult<()> {
        let task = self.task(task_id)?;
        if task.project_id != project_id {
            return Err(NovaError::Config(
                "证据的 project_id 与任务所属项目不一致，已拒绝写入".to_string(),
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_timeline_event(
        &self,
        project_id: &str,
        task_id: Option<&str>,
        phase: Option<&str>,
        event_type: &str,
        summary: &str,
        detail: Option<&str>,
        source: &str,
        correlation_id: Option<&str>,
    ) -> NovaResult<TimelineEvent> {
        if let Some(task_id) = task_id {
            self.assert_task_belongs_to_project(project_id, task_id)?;
        }
        let id = new_id();
        let now = now();
        let summary = redact_sensitive_text(summary);
        let detail = detail.map(redact_sensitive_text);
        self.conn().execute(
            "INSERT INTO timeline_events (id, project_id, task_id, phase, event_type, summary, detail, source, correlation_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id, project_id, task_id, phase, event_type, summary, detail, source, correlation_id, now],
        )?;
        self.timeline_event(&id)
    }

    fn timeline_event(&self, id: &str) -> NovaResult<TimelineEvent> {
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, phase, event_type, summary, detail, source, correlation_id, created_at
                 FROM timeline_events WHERE id = ?1",
                params![id],
                read_timeline_event,
            )
            .map_err(Into::into)
    }

    pub fn list_timeline_events_for_project(
        &self,
        project_id: &str,
    ) -> NovaResult<Vec<TimelineEvent>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, phase, event_type, summary, detail, source, correlation_id, created_at
             FROM timeline_events WHERE project_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![project_id], read_timeline_event)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_command_evidence(
        &self,
        project_id: &str,
        task_id: &str,
        command_redacted: &str,
        exit_code: Option<i64>,
        started_at: Option<&str>,
        finished_at: Option<&str>,
        stdout_ref: Option<&str>,
        stderr_ref: Option<&str>,
    ) -> NovaResult<CommandEvidence> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let id = new_id();
        let now = now();
        let command_redacted = redact_sensitive_text(command_redacted);
        self.conn().execute(
            "INSERT INTO command_evidence (id, project_id, task_id, command_redacted, exit_code, started_at, finished_at, stdout_ref, stderr_ref, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id, project_id, task_id, command_redacted, exit_code, started_at, finished_at, stdout_ref, stderr_ref, now],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, command_redacted, exit_code, started_at, finished_at, stdout_ref, stderr_ref, created_at
                 FROM command_evidence WHERE id = ?1",
                params![id],
                read_command_evidence,
            )
            .map_err(Into::into)
    }

    pub fn list_command_evidence_for_task(
        &self,
        task_id: &str,
    ) -> NovaResult<Vec<CommandEvidence>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, command_redacted, exit_code, started_at, finished_at, stdout_ref, stderr_ref, created_at
             FROM command_evidence WHERE task_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id], read_command_evidence)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn record_git_evidence(
        &self,
        project_id: &str,
        task_id: &str,
        event_kind: &str,
        commit_sha: Option<&str>,
        branch: Option<&str>,
        diff_stat: Option<&str>,
    ) -> NovaResult<GitEvidence> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let id = new_id();
        let now = now();
        self.conn().execute(
            "INSERT INTO git_evidence (id, project_id, task_id, event_kind, commit_sha, branch, diff_stat, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, project_id, task_id, event_kind, commit_sha, branch, diff_stat, now],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, event_kind, commit_sha, branch, diff_stat, created_at FROM git_evidence WHERE id = ?1",
                params![id],
                read_git_evidence,
            )
            .map_err(Into::into)
    }

    pub fn record_file_change_evidence(
        &self,
        project_id: &str,
        task_id: &str,
        file_path: &str,
        change_kind: &str,
        summary: Option<&str>,
    ) -> NovaResult<FileChangeEvidence> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        if !matches!(change_kind, "created" | "modified" | "deleted") {
            return Err(NovaError::Config(format!(
                "未知的文件变更类型：{change_kind}"
            )));
        }
        let id = new_id();
        let now = now();
        self.conn().execute(
            "INSERT INTO file_change_evidence (id, project_id, task_id, file_path, change_kind, summary, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, project_id, task_id, file_path, change_kind, summary, now],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, file_path, change_kind, summary, created_at FROM file_change_evidence WHERE id = ?1",
                params![id],
                read_file_change_evidence,
            )
            .map_err(Into::into)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_test_evidence(
        &self,
        project_id: &str,
        task_id: &str,
        test_command_redacted: &str,
        passed_count: i64,
        failed_count: i64,
        skipped_count: i64,
        exit_code: Option<i64>,
    ) -> NovaResult<TestEvidence> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let id = new_id();
        let now = now();
        let test_command_redacted = redact_sensitive_text(test_command_redacted);
        self.conn().execute(
            "INSERT INTO test_evidence (id, project_id, task_id, test_command_redacted, passed_count, failed_count, skipped_count, exit_code, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, project_id, task_id, test_command_redacted, passed_count, failed_count, skipped_count, exit_code, now],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, test_command_redacted, passed_count, failed_count, skipped_count, exit_code, created_at
                 FROM test_evidence WHERE id = ?1",
                params![id],
                read_test_evidence,
            )
            .map_err(Into::into)
    }

    /// Records the user's explicit accept/reject decision for a task's
    /// *current* result. This must only ever be called from the Tauri
    /// command wired to the literal "我已经看过，符合需要" confirmation
    /// action in the UI — never from `nova`/`connected_ai`/`system` code
    /// paths — so that [`Database::transition_task`]'s `completed` gate
    /// can never be satisfied by anything other than a real user action.
    ///
    /// The write is only accepted while the task is currently
    /// `awaiting_user_acceptance`, and it is stamped with the task's
    /// current `result_generation` — both facts are read from `tasks`
    /// inside the very same `INSERT ... SELECT ... WHERE` statement that
    /// performs the write, so there is no window between "check state" and
    /// "write" for a concurrent `transition_task` call to invalidate the
    /// decision (e.g. by moving the task to `safe_fixing`/bumping the
    /// generation in between). One row per `(task_id, result_generation)`;
    /// calling again for the same generation updates that row rather than
    /// creating a second one — a task can freely be re-accepted/re-rejected
    /// while still on the same result, but a *new* result (new generation)
    /// always starts with no acceptance row of its own, never inheriting
    /// the old generation's verdict.
    pub fn record_user_acceptance(
        &self,
        project_id: &str,
        task_id: &str,
        accepted: bool,
        user_note: Option<&str>,
    ) -> NovaResult<UserAcceptance> {
        let user_note = user_note.map(redact_sensitive_text);
        let now = now();
        let rows_affected = self.conn().execute(
            "INSERT INTO user_acceptance (id, project_id, task_id, result_generation, accepted, user_note, accepted_at, created_at)
             SELECT ?1, project_id, id, result_generation, ?2, ?3, ?4, ?4
             FROM tasks
             WHERE id = ?5 AND project_id = ?6 AND state = 'awaiting_user_acceptance'
             ON CONFLICT(task_id, result_generation) DO UPDATE SET
               accepted = excluded.accepted,
               user_note = excluded.user_note,
               accepted_at = excluded.accepted_at",
            params![new_id(), accepted as i64, user_note, now, task_id, project_id],
        )?;

        if rows_affected == 0 {
            // Diagnose why, purely for a clear error message.
            let task = self.task(task_id)?;
            if task.project_id != project_id {
                return Err(NovaError::Config(
                    "证据的 project_id 与任务所属项目不一致，已拒绝写入".to_string(),
                ));
            }
            return Err(NovaError::Config(format!(
                "只有任务处于 awaiting_user_acceptance 状态时才能记录验收，当前状态：{}",
                task.state
            )));
        }

        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, accepted, user_note, accepted_at, created_at
                 FROM user_acceptance WHERE task_id = ?1 ORDER BY result_generation DESC LIMIT 1",
                params![task_id],
                read_user_acceptance,
            )
            .map_err(Into::into)
    }

    /// The acceptance row for a task's *current* `result_generation`, if
    /// one exists — this is what `completed` actually depends on. Older
    /// generations' rows still exist for audit but are deliberately not
    /// returned here, since they can never authorize the current result.
    pub fn user_acceptance_for_task(&self, task_id: &str) -> NovaResult<Option<UserAcceptance>> {
        self.conn()
            .query_row(
                "SELECT ua.id, ua.project_id, ua.task_id, ua.result_generation, ua.accepted, ua.user_note, ua.accepted_at, ua.created_at
                 FROM user_acceptance ua
                 JOIN tasks t ON t.id = ua.task_id
                 WHERE ua.task_id = ?1 AND ua.result_generation = t.result_generation",
                params![task_id],
                read_user_acceptance,
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other.into()),
            })
    }

    pub fn record_generated_document(
        &self,
        project_id: &str,
        task_id: Option<&str>,
        document_type: &str,
        file_path: Option<&str>,
        generated_by: Option<&str>,
    ) -> NovaResult<GeneratedDocument> {
        if let Some(task_id) = task_id {
            self.assert_task_belongs_to_project(project_id, task_id)?;
        }
        let id = new_id();
        let now = now();
        self.conn().execute(
            "INSERT INTO generated_documents (id, project_id, task_id, document_type, file_path, generated_by, generated_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![id, project_id, task_id, document_type, file_path, generated_by, now],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, document_type, file_path, generated_by, generated_at, created_at FROM generated_documents WHERE id = ?1",
                params![id],
                read_generated_document,
            )
            .map_err(Into::into)
    }

    /// Links a piece of evidence (any of the tables above) to a timeline
    /// event. `evidence_type` is a free-form tag (`"command"`, `"git"`,
    /// `"file_change"`, `"test"`) resolved by callers, not a foreign key —
    /// evidence rows live in different tables, so this is a soft link by
    /// design, matching `evidence_refs`' role as a cross-table index rather
    /// than a referential-integrity boundary.
    pub fn link_evidence_ref(
        &self,
        event_id: &str,
        evidence_type: &str,
        evidence_id: &str,
    ) -> NovaResult<()> {
        self.conn().execute(
            "INSERT INTO evidence_refs (id, event_id, evidence_type, evidence_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![new_id(), event_id, evidence_type, evidence_id, now()],
        )?;
        Ok(())
    }
}

fn read_timeline_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<TimelineEvent> {
    Ok(TimelineEvent {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        phase: row.get(3)?,
        event_type: row.get(4)?,
        summary: row.get(5)?,
        detail: row.get(6)?,
        source: row.get(7)?,
        correlation_id: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn read_command_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<CommandEvidence> {
    Ok(CommandEvidence {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        command_redacted: row.get(3)?,
        exit_code: row.get(4)?,
        started_at: row.get(5)?,
        finished_at: row.get(6)?,
        stdout_ref: row.get(7)?,
        stderr_ref: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn read_git_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<GitEvidence> {
    Ok(GitEvidence {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        event_kind: row.get(3)?,
        commit_sha: row.get(4)?,
        branch: row.get(5)?,
        diff_stat: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn read_file_change_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileChangeEvidence> {
    Ok(FileChangeEvidence {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        file_path: row.get(3)?,
        change_kind: row.get(4)?,
        summary: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn read_test_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<TestEvidence> {
    Ok(TestEvidence {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        test_command_redacted: row.get(3)?,
        passed_count: row.get(4)?,
        failed_count: row.get(5)?,
        skipped_count: row.get(6)?,
        exit_code: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn read_user_acceptance(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserAcceptance> {
    let accepted: i64 = row.get(4)?;
    Ok(UserAcceptance {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        result_generation: row.get(3)?,
        accepted: accepted != 0,
        user_note: row.get(5)?,
        accepted_at: row.get(6)?,
        created_at: row.get(7)?,
    })
}

fn read_generated_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<GeneratedDocument> {
    Ok(GeneratedDocument {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        document_type: row.get(3)?,
        file_path: row.get(4)?,
        generated_by: row.get(5)?,
        generated_at: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Actor, CreateProjectRequest, CreateTaskRequest, TaskState};
    use std::fs;
    use std::path::PathBuf;

    /// Drives a freshly-created (`draft`) task all the way to
    /// `awaiting_user_acceptance`, the only state `record_user_acceptance`
    /// will accept a write in.
    fn drive_to_awaiting_acceptance(db: &Database, task_id: &str) {
        for state in [
            TaskState::AwaitingConfirmation,
            TaskState::Confirmed,
            TaskState::Queued,
            TaskState::Running,
            TaskState::Verifying,
            TaskState::PreviewReady,
            TaskState::AwaitingUserAcceptance,
        ] {
            db.transition_task(task_id, state, Actor::System, None, None, false, None)
                .unwrap();
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-evidence-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db_with_task(dir: &std::path::Path, name: &str) -> (Database, String, String) {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let project = db
            .create_project(&CreateProjectRequest {
                name: name.to_string(),
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
    fn evidence_is_scoped_to_its_project_and_task() {
        let dir = temp_dir("scoped");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        db.record_command_evidence(
            &project_id,
            &task_id,
            "npm test",
            Some(0),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let list = db.list_command_evidence_for_task(&task_id).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].project_id, project_id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evidence_referencing_a_task_from_a_different_project_is_rejected() {
        let dir_a = temp_dir("cross-a");
        let dir_b = temp_dir("cross-b");
        let (db, _project_a, task_a) = db_with_task(&dir_a, "A");
        let (_db_unused, project_b, _task_b) = db_with_task(&dir_b, "B");
        // Reuse `db` (same connection as project A) but pass project B's id
        // alongside project A's task — this must be rejected.
        let result = db
            .record_command_evidence(&project_b, &task_a, "echo hi", None, None, None, None, None);
        assert!(
            result.is_err(),
            "evidence must not be attributable to a project that doesn't own the task"
        );
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn command_evidence_text_is_redacted_before_storage() {
        let dir = temp_dir("redacted-command");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        let evidence = db
            .record_command_evidence(
                &project_id,
                &task_id,
                "curl -H 'Authorization: Bearer sk-secret-value-1234567890' https://api.example.com",
                Some(0),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(!evidence
            .command_redacted
            .contains("sk-secret-value-1234567890"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_acceptance_is_uniquely_associated_with_one_task() {
        let dir = temp_dir("unique-acceptance");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        drive_to_awaiting_acceptance(&db, &task_id);
        db.record_user_acceptance(&project_id, &task_id, true, Some("看起来不错"))
            .unwrap();
        db.record_user_acceptance(&project_id, &task_id, true, Some("再次确认"))
            .unwrap();

        let acceptance = db.user_acceptance_for_task(&task_id).unwrap().unwrap();
        assert_eq!(acceptance.user_note.as_deref(), Some("再次确认"));

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM user_acceptance WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "a task must have at most one acceptance row per result_generation"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn acceptance_note_is_redacted_before_storage() {
        let dir = temp_dir("redact-acceptance-note");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        drive_to_awaiting_acceptance(&db, &task_id);
        db.record_user_acceptance(
            &project_id,
            &task_id,
            true,
            Some("token=abc123supersecretvalue"),
        )
        .unwrap();

        let acceptance = db.user_acceptance_for_task(&task_id).unwrap().unwrap();
        let note = acceptance.user_note.as_deref().unwrap();
        assert!(
            !note.contains("abc123supersecretvalue"),
            "acceptance note must be redacted before storage: {note}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_completion_record_exists_without_an_explicit_acceptance_call() {
        let dir = temp_dir("no-fake-acceptance");
        let (db, _project_id, task_id) = db_with_task(&dir, "P");
        let acceptance = db.user_acceptance_for_task(&task_id).unwrap();
        assert!(
            acceptance.is_none(),
            "nothing must fabricate a user_acceptance row on its own"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timeline_events_scoped_by_project_do_not_leak_other_projects_rows() {
        let dir_a = temp_dir("timeline-a");
        let dir_b = temp_dir("timeline-b");
        let (db, project_a, task_a) = db_with_task(&dir_a, "A");
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
            .unwrap()
            .id;

        db.record_timeline_event(
            &project_a,
            Some(&task_a),
            None,
            "FILE_CHANGED",
            "改了一个文件",
            None,
            "nova_collected",
            None,
        )
        .unwrap();
        db.record_timeline_event(
            &project_b,
            None,
            None,
            "PLAN_CREATED",
            "项目B的计划",
            None,
            "nova_collected",
            None,
        )
        .unwrap();

        let events_a = db.list_timeline_events_for_project(&project_a).unwrap();
        assert_eq!(events_a.len(), 1);
        assert!(events_a.iter().all(|e| e.project_id == project_a));
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn file_change_evidence_rejects_unknown_change_kind() {
        let dir = temp_dir("bad-change-kind");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        let result =
            db.record_file_change_evidence(&project_id, &task_id, "src/main.rs", "renamed", None);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_evidence_round_trips() {
        let dir = temp_dir("git-evidence");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        let evidence = db
            .record_git_evidence(
                &project_id,
                &task_id,
                "commit",
                Some("abc123"),
                Some("feature/x"),
                Some("2 files changed"),
            )
            .unwrap();
        assert_eq!(evidence.commit_sha.as_deref(), Some("abc123"));
        assert_eq!(evidence.task_id, task_id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_evidence_records_pass_fail_skip_counts() {
        let dir = temp_dir("test-evidence");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        let evidence = db
            .record_test_evidence(&project_id, &task_id, "cargo test", 125, 0, 0, Some(0))
            .unwrap();
        assert_eq!(evidence.passed_count, 125);
        assert_eq!(evidence.exit_code, Some(0));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_document_can_be_recorded_without_a_task() {
        let dir = temp_dir("generated-doc");
        let (db, project_id, _task_id) = db_with_task(&dir, "P");
        let doc = db
            .record_generated_document(
                &project_id,
                None,
                "PRODUCT_CONTEXT",
                Some(".nova/memory/PRODUCT_CONTEXT.md"),
                Some("nova"),
            )
            .unwrap();
        assert_eq!(doc.document_type, "PRODUCT_CONTEXT");
        assert!(doc.task_id.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn evidence_refs_link_a_timeline_event_to_underlying_evidence() {
        let dir = temp_dir("evidence-refs");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        let event = db
            .record_timeline_event(
                &project_id,
                Some(&task_id),
                None,
                "TEST_FINISHED",
                "测试完成",
                None,
                "cli",
                None,
            )
            .unwrap();
        let test_evidence = db
            .record_test_evidence(&project_id, &task_id, "cargo test", 10, 0, 0, Some(0))
            .unwrap();
        db.link_evidence_ref(&event.id, "test", &test_evidence.id)
            .unwrap();

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM evidence_refs WHERE event_id = ?1",
                params![event.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
