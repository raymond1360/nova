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
    BuildEvidence, CommandEvidence, FileChangeEvidence, GeneratedDocument, GitEvidence,
    TestEvidence, TimelineEvent, UserAcceptance,
};
use crate::security::redact_sensitive_text;

use super::{new_id, now};

/// Parameters for [`Database::record_test_evidence`] — a struct rather than
/// a long positional argument list, since Blocker 3 needs considerably more
/// fields than the original Phase 2 stub did.
pub struct RecordTestEvidenceRequest<'a> {
    pub project_id: &'a str,
    pub task_id: &'a str,
    pub result_generation: i64,
    pub round: i64,
    pub test_command_redacted: Option<&'a str>,
    pub status: &'a str,
    pub passed_count: i64,
    pub failed_count: i64,
    pub skipped_count: i64,
    pub exit_code: Option<i64>,
    pub reason: Option<&'a str>,
    pub stdout_ref: Option<&'a str>,
    pub stderr_ref: Option<&'a str>,
}

/// Codex re-review remediation (Major 3): parameters for
/// [`Database::record_build_evidence`] — mirrors
/// [`RecordTestEvidenceRequest`] exactly, one field shorter (no
/// pass/fail/skip counts, since a build has no such concept).
pub struct RecordBuildEvidenceRequest<'a> {
    pub project_id: &'a str,
    pub task_id: &'a str,
    pub result_generation: i64,
    pub round: i64,
    pub build_command_redacted: Option<&'a str>,
    pub status: &'a str,
    pub exit_code: Option<i64>,
    pub reason: Option<&'a str>,
    pub stdout_ref: Option<&'a str>,
    pub stderr_ref: Option<&'a str>,
}

impl Database {
    /// Fails if `task_id` does not belong to `project_id` — the guard every
    /// evidence-writing function below runs first, so evidence can never be
    /// attributed to the wrong project even if a caller passes a mismatched
    /// pair by mistake.
    pub(super) fn assert_task_belongs_to_project(
        &self,
        project_id: &str,
        task_id: &str,
    ) -> NovaResult<()> {
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

    /// Phase 4 (evidence/document gate): every Git fact recorded for a
    /// task, oldest first — read-only counterpart to
    /// [`Database::record_git_evidence`].
    pub fn list_git_evidence_for_task(&self, task_id: &str) -> NovaResult<Vec<GitEvidence>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, event_kind, commit_sha, branch, diff_stat, created_at
             FROM git_evidence WHERE task_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id], read_git_evidence)?
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

    /// Phase 4 (evidence/document gate): every file-change fact recorded
    /// for a task, oldest first — read-only counterpart to
    /// [`Database::record_file_change_evidence`].
    pub fn list_file_change_evidence_for_task(
        &self,
        task_id: &str,
    ) -> NovaResult<Vec<FileChangeEvidence>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, file_path, change_kind, summary, created_at
             FROM file_change_evidence WHERE task_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id], read_file_change_evidence)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Phase 3 remediation (Blocker 3): records one independently-run test
    /// attempt for a task's *current* `(result_generation, round)`. Every
    /// field is exactly what Nova itself observed running the command
    /// through `runtime::process` — never anything an AI merely claimed.
    /// `test_command_redacted`/`reason`/`stdout_ref`/`stderr_ref` are
    /// redacted before storage, same as every other evidence-writing
    /// function in this module.
    ///
    /// Phase 4 remediation (Major 2, write-layer defense in depth): a
    /// `no_tests_applicable` row may only ever be written once a real
    /// `no_tests_confirmations` row already exists for this *exact*
    /// `(project_id, task_id, result_generation, round)` — mirroring the
    /// check `core_loop::run_tests_for_round` (the only production caller
    /// of this status) already performs immediately before calling this
    /// function. Enforcing it here too means no future caller, test, or
    /// refactor can accidentally persist an unconfirmed "not applicable"
    /// row that the read/completion-gate layers would then have to treat
    /// as legitimate. Every other status is unaffected.
    pub fn record_test_evidence(
        &self,
        request: &RecordTestEvidenceRequest<'_>,
    ) -> NovaResult<TestEvidence> {
        self.assert_task_belongs_to_project(request.project_id, request.task_id)?;
        if request.status == "no_tests_applicable"
            && self
                .find_no_tests_confirmation(
                    request.project_id,
                    request.task_id,
                    request.result_generation,
                    request.round,
                )?
                .is_none()
        {
            return Err(NovaError::Config(
                "拒绝写入 no_tests_applicable：当前 generation/round 不存在匹配的无测试确认记录"
                    .to_string(),
            ));
        }
        let id = new_id();
        let now = now();
        let test_command_redacted = request.test_command_redacted.map(redact_sensitive_text);
        let reason = request.reason.map(redact_sensitive_text);
        let stdout_ref = request.stdout_ref.map(redact_sensitive_text);
        let stderr_ref = request.stderr_ref.map(redact_sensitive_text);
        self.conn().execute(
            "INSERT INTO test_evidence (id, project_id, task_id, result_generation, round, test_command_redacted, status, passed_count, failed_count, skipped_count, exit_code, reason, stdout_ref, stderr_ref, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                id,
                request.project_id,
                request.task_id,
                request.result_generation,
                request.round,
                test_command_redacted,
                request.status,
                request.passed_count,
                request.failed_count,
                request.skipped_count,
                request.exit_code,
                reason,
                stdout_ref,
                stderr_ref,
                now
            ],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, round, test_command_redacted, status, passed_count, failed_count, skipped_count, exit_code, reason, stdout_ref, stderr_ref, created_at
                 FROM test_evidence WHERE id = ?1",
                params![id],
                read_test_evidence,
            )
            .map_err(Into::into)
    }

    /// Every test-evidence row for a task's *current* `(result_generation,
    /// round)` — a review may only ever consider evidence matching this
    /// exact pair, never a stale earlier round's (Blocker 3 requirement).
    pub fn list_test_evidence_for_round(
        &self,
        project_id: &str,
        task_id: &str,
        result_generation: i64,
        round: i64,
    ) -> NovaResult<Vec<TestEvidence>> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, result_generation, round, test_command_redacted, status, passed_count, failed_count, skipped_count, exit_code, reason, stdout_ref, stderr_ref, created_at
             FROM test_evidence WHERE task_id = ?1 AND result_generation = ?2 AND round = ?3 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(
                params![task_id, result_generation, round],
                read_test_evidence,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Codex re-review remediation (Major 3): records one independently-run
    /// build attempt for a task's *current* `(result_generation, round)` —
    /// mirrors [`Database::record_test_evidence`] exactly, including the
    /// same redaction discipline for every text field.
    ///
    /// Phase 4 remediation (Major 2): deliberately does **not** gain the
    /// same write-time "a matching confirmation must already exist" guard
    /// [`Database::record_test_evidence`] does — audited production call
    /// chain (`core_loop::run_build_for_round`) shows `no_build_applicable`
    /// has two distinct legitimate origins: (a) `BuildDetection::
    /// Insufficient` resolved via a real, persisted `no_build_confirmation`
    /// (mirrors the test case exactly), and (b) `BuildDetection::
    /// NotApplicable` — Nova's own mechanical, deterministic determination
    /// from real project metadata (e.g. `package.json` with no `build`
    /// script) that a build genuinely never applies, which by design has
    /// never required or written a confirmation row at all. A write-time
    /// guard requiring a confirmation for every `no_build_applicable` row
    /// would reject case (b) outright — a real functional regression,
    /// proven by three existing single-AI closed-loop tests failing when
    /// this guard was attempted. See `core_loop::documents::
    /// no_build_applicable_is_authorized` for where this asymmetry is
    /// instead enforced correctly, at the read/completion-gate layer, which
    /// can re-derive case (b)'s legitimacy directly from the project
    /// directory rather than assuming every unconfirmed row is illegitimate.
    pub fn record_build_evidence(
        &self,
        request: &RecordBuildEvidenceRequest<'_>,
    ) -> NovaResult<BuildEvidence> {
        self.assert_task_belongs_to_project(request.project_id, request.task_id)?;
        let id = new_id();
        let now = now();
        let build_command_redacted = request.build_command_redacted.map(redact_sensitive_text);
        let reason = request.reason.map(redact_sensitive_text);
        let stdout_ref = request.stdout_ref.map(redact_sensitive_text);
        let stderr_ref = request.stderr_ref.map(redact_sensitive_text);
        self.conn().execute(
            "INSERT INTO build_evidence (id, project_id, task_id, result_generation, round, build_command_redacted, status, exit_code, reason, stdout_ref, stderr_ref, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                id,
                request.project_id,
                request.task_id,
                request.result_generation,
                request.round,
                build_command_redacted,
                request.status,
                request.exit_code,
                reason,
                stdout_ref,
                stderr_ref,
                now
            ],
        )?;
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, result_generation, round, build_command_redacted, status, exit_code, reason, stdout_ref, stderr_ref, created_at
                 FROM build_evidence WHERE id = ?1",
                params![id],
                read_build_evidence,
            )
            .map_err(Into::into)
    }

    /// Every build-evidence row for a task's *current* `(result_generation,
    /// round)` — mirrors [`Database::list_test_evidence_for_round`] exactly.
    pub fn list_build_evidence_for_round(
        &self,
        project_id: &str,
        task_id: &str,
        result_generation: i64,
        round: i64,
    ) -> NovaResult<Vec<BuildEvidence>> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, result_generation, round, build_command_redacted, status, exit_code, reason, stdout_ref, stderr_ref, created_at
             FROM build_evidence WHERE task_id = ?1 AND result_generation = ?2 AND round = ?3 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(
                params![task_id, result_generation, round],
                read_build_evidence,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
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

    /// Phase 4 (evidence/document gate): every acceptance/rejection row a
    /// task has ever accumulated, oldest generation first — unlike
    /// [`Database::user_acceptance_for_task`] (which deliberately returns
    /// only the *current*-generation row, the one `completed` actually
    /// depends on), this is a read-only audit view for the evidence
    /// timeline. Older rows are real, already-recorded facts and are never
    /// hidden, but callers must never treat anything returned here as
    /// authorizing completion for a generation other than the task's
    /// current one — that decision stays exclusively with
    /// `user_acceptance_for_task`.
    pub fn list_user_acceptance_for_task(
        &self,
        project_id: &str,
        task_id: &str,
    ) -> NovaResult<Vec<UserAcceptance>> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, result_generation, accepted, user_note, accepted_at, created_at
             FROM user_acceptance WHERE task_id = ?1 ORDER BY result_generation ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id], read_user_acceptance)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
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
        result_generation: row.get(3)?,
        round: row.get(4)?,
        test_command_redacted: row.get(5)?,
        status: row.get(6)?,
        passed_count: row.get(7)?,
        failed_count: row.get(8)?,
        skipped_count: row.get(9)?,
        exit_code: row.get(10)?,
        reason: row.get(11)?,
        stdout_ref: row.get(12)?,
        stderr_ref: row.get(13)?,
        created_at: row.get(14)?,
    })
}

fn read_build_evidence(row: &rusqlite::Row<'_>) -> rusqlite::Result<BuildEvidence> {
    Ok(BuildEvidence {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        result_generation: row.get(3)?,
        round: row.get(4)?,
        build_command_redacted: row.get(5)?,
        status: row.get(6)?,
        exit_code: row.get(7)?,
        reason: row.get(8)?,
        stdout_ref: row.get(9)?,
        stderr_ref: row.get(10)?,
        created_at: row.get(11)?,
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
    fn list_user_acceptance_for_task_returns_every_generation_in_order() {
        let dir = temp_dir("list-acceptance-history");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        drive_to_awaiting_acceptance(&db, &task_id);
        db.record_user_acceptance(&project_id, &task_id, false, Some("第一次拒绝"))
            .unwrap();
        // Bump the generation the same way a real fix-and-requeue cycle
        // does, then accept the new generation — mirrors how a real task's
        // history accumulates more than one acceptance row over its
        // lifetime.
        db.transition_task(
            &task_id,
            TaskState::NeedsUserAction,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        // Transitioning back through to a fresh `preview_ready` is what
        // actually bumps `result_generation` (see
        // `storage::tasks::transition_task`) — mirrors how a real
        // fix-and-requeue cycle produces a second, distinct generation.
        for state in [
            TaskState::Queued,
            TaskState::Running,
            TaskState::Verifying,
            TaskState::PreviewReady,
            TaskState::AwaitingUserAcceptance,
        ] {
            db.transition_task(&task_id, state, Actor::System, None, None, false, None)
                .unwrap();
        }
        db.record_user_acceptance(&project_id, &task_id, true, Some("第二次通过"))
            .unwrap();

        let history = db
            .list_user_acceptance_for_task(&project_id, &task_id)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert!(!history[0].accepted);
        assert_eq!(history[0].user_note.as_deref(), Some("第一次拒绝"));
        assert!(history[1].accepted);
        assert_eq!(history[1].user_note.as_deref(), Some("第二次通过"));
        assert!(history[0].result_generation < history[1].result_generation);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_user_acceptance_for_task_rejects_a_project_id_that_does_not_own_the_task() {
        let dir_a = temp_dir("list-acceptance-wrong-project-a");
        let dir_b = temp_dir("list-acceptance-wrong-project-b");
        let (db, _project_a, task_a) = db_with_task(&dir_a, "A");
        let (_db_unused, project_b, _task_b) = db_with_task(&dir_b, "B");
        let result = db.list_user_acceptance_for_task(&project_b, &task_a);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
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
            .record_test_evidence(&RecordTestEvidenceRequest {
                project_id: &project_id,
                task_id: &task_id,
                result_generation: 0,
                round: 1,
                test_command_redacted: Some("cargo test"),
                status: "passed",
                passed_count: 125,
                failed_count: 0,
                skipped_count: 0,
                exit_code: Some(0),
                reason: None,
                stdout_ref: None,
                stderr_ref: None,
            })
            .unwrap();
        assert_eq!(evidence.passed_count, 125);
        assert_eq!(evidence.exit_code, Some(0));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_evidence_records_status_and_exit_code() {
        let dir = temp_dir("build-evidence");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        let evidence = db
            .record_build_evidence(&RecordBuildEvidenceRequest {
                project_id: &project_id,
                task_id: &task_id,
                result_generation: 0,
                round: 1,
                build_command_redacted: Some("cargo build"),
                status: "passed",
                exit_code: Some(0),
                reason: None,
                stdout_ref: None,
                stderr_ref: None,
            })
            .unwrap();
        assert_eq!(evidence.status, "passed");
        assert_eq!(evidence.exit_code, Some(0));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_evidence_text_fields_are_redacted_before_storage() {
        let dir = temp_dir("build-evidence-redact");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        let evidence = db
            .record_build_evidence(&RecordBuildEvidenceRequest {
                project_id: &project_id,
                task_id: &task_id,
                result_generation: 0,
                round: 1,
                build_command_redacted: Some("npm run build"),
                status: "failed",
                exit_code: Some(1),
                reason: Some("api_key=sk-1234567890abcdef1234567890abcdef build failed"),
                stdout_ref: Some("sk-1234567890abcdef1234567890abcdef in stdout"),
                stderr_ref: None,
            })
            .unwrap();
        assert!(!evidence
            .reason
            .unwrap()
            .contains("sk-1234567890abcdef1234567890abcdef"));
        assert!(!evidence
            .stdout_ref
            .unwrap()
            .contains("sk-1234567890abcdef1234567890abcdef"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_build_evidence_for_round_is_scoped_to_the_exact_generation_and_round() {
        let dir = temp_dir("build-evidence-scoped");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        db.record_build_evidence(&RecordBuildEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            build_command_redacted: Some("cargo build"),
            status: "passed",
            exit_code: Some(0),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        db.record_build_evidence(&RecordBuildEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 2,
            build_command_redacted: Some("cargo build"),
            status: "failed",
            exit_code: Some(1),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();

        let round1 = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(round1.len(), 1);
        assert_eq!(round1[0].status, "passed");
        let round2 = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 2)
            .unwrap();
        assert_eq!(round2.len(), 1);
        assert_eq!(round2[0].status, "failed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_evidence_referencing_a_task_from_a_different_project_is_rejected() {
        let dir_a = temp_dir("build-cross-a");
        let dir_b = temp_dir("build-cross-b");
        let (db, _project_a, task_a) = db_with_task(&dir_a, "A");
        let (_db_unused, project_b, _task_b) = db_with_task(&dir_b, "B");
        let result = db.record_build_evidence(&RecordBuildEvidenceRequest {
            project_id: &project_b,
            task_id: &task_a,
            result_generation: 0,
            round: 1,
            build_command_redacted: Some("cargo build"),
            status: "passed",
            exit_code: Some(0),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        });
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
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
            .record_test_evidence(&RecordTestEvidenceRequest {
                project_id: &project_id,
                task_id: &task_id,
                result_generation: 0,
                round: 1,
                test_command_redacted: Some("cargo test"),
                status: "passed",
                passed_count: 10,
                failed_count: 0,
                skipped_count: 0,
                exit_code: Some(0),
                reason: None,
                stdout_ref: None,
                stderr_ref: None,
            })
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
