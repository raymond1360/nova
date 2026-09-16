//! Idempotent action ledger (Phase 2, section E): builds and tests the
//! control mechanism that will gate real destructive operations once
//! Phase 3's execution loop exists. Per scope, Phase 2 does not actually
//! execute any such action, so nothing here is reachable from a Tauri
//! command yet — `mark_started_actions_uncertain` is the sole exception
//! (called from `storage::recovery`). Everything else is exercised only by
//! this module's own `#[cfg(test)]` suite for now.
#![allow(dead_code)]

use rusqlite::{params, OptionalExtension};

use crate::db::Database;
use crate::error::{NovaError, NovaResult};
use crate::models::{ActionStatus, TaskAction};

use super::tasks::parse_enum;
use super::{new_id, now};

impl Database {
    /// Records the intent to perform `action_type` under `idempotency_key`.
    /// Calling this again for a key that already has a row is a no-op that
    /// returns the existing row unchanged — planning is itself idempotent,
    /// including under a genuine race between two connections (e.g. two
    /// Nova processes): the check-then-insert below has a TOCTOU window,
    /// but `UNIQUE(task_id, idempotency_key)` is the real backstop — if a
    /// concurrent caller's INSERT wins that race, this one's INSERT fails
    /// with a unique-constraint violation, which is treated as "someone
    /// else already planned it" and resolved by reading their row back,
    /// not propagated as an error.
    pub fn plan_action(
        &self,
        task_id: &str,
        action_type: &str,
        idempotency_key: &str,
    ) -> NovaResult<TaskAction> {
        if let Some(existing) = self.find_action(task_id, idempotency_key)? {
            return Ok(existing);
        }
        let id = new_id();
        let now = now();
        let insert_result = self.conn().execute(
            "INSERT INTO task_actions (id, task_id, action_type, idempotency_key, attempt_number, status, started_at, finished_at, evidence_ref, error_summary, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, NULL, NULL, NULL, NULL, ?6, ?6)",
            params![id, task_id, action_type, idempotency_key, ActionStatus::Planned.as_str(), now],
        );
        if let Err(error) = &insert_result {
            if !is_unique_constraint_violation(error) {
                insert_result?;
            }
        }
        self.find_action(task_id, idempotency_key)?
            .ok_or_else(|| NovaError::Database("刚插入的动作记录未能读回".to_string()))
    }

    /// Marks the action as started via a single conditional `UPDATE`, not a
    /// read followed by a write: the legality check (which prior statuses
    /// may become `started`) is part of the `WHERE` clause itself, so the
    /// decision and the write happen as one atomic statement. Under two
    /// concurrent connections racing the same non-read-only action, at
    /// most one `UPDATE` can match a row still in an eligible prior status
    /// — the loser's `rows_affected` is `0`, never a second silent
    /// "success". Non-read-only actions may only start from `planned`;
    /// read-only actions (`is_read_only = true`) may also (re)start from
    /// any terminal status, including `uncertain` (their retries are safe
    /// by definition — see [`Database::acknowledge_uncertain_action`] for
    /// the non-read-only equivalent, which is required first).
    pub fn start_action(
        &self,
        task_id: &str,
        idempotency_key: &str,
        is_read_only: bool,
    ) -> NovaResult<TaskAction> {
        let now = now();
        let rows_affected = if is_read_only {
            self.conn().execute(
                "UPDATE task_actions
                 SET status = 'started',
                     started_at = ?1,
                     finished_at = NULL,
                     attempt_number = CASE WHEN status = 'planned' THEN attempt_number ELSE attempt_number + 1 END,
                     updated_at = ?1
                 WHERE task_id = ?2 AND idempotency_key = ?3
                   AND status IN ('planned', 'succeeded', 'failed', 'cancelled', 'uncertain')",
                params![now, task_id, idempotency_key],
            )?
        } else {
            self.conn().execute(
                "UPDATE task_actions
                 SET status = 'started', started_at = ?1, finished_at = NULL, updated_at = ?1
                 WHERE task_id = ?2 AND idempotency_key = ?3 AND status = 'planned'",
                params![now, task_id, idempotency_key],
            )?
        };

        if rows_affected == 0 {
            // Diagnose why, purely for a clear error message — this read
            // is not authoritative for the decision, which was already
            // made atomically above.
            return Err(self.start_action_conflict_message(
                task_id,
                idempotency_key,
                is_read_only,
            )?);
        }

        self.find_action(task_id, idempotency_key)?
            .ok_or_else(|| NovaError::Database("动作记录在更新后丢失".to_string()))
    }

    fn start_action_conflict_message(
        &self,
        task_id: &str,
        idempotency_key: &str,
        is_read_only: bool,
    ) -> NovaResult<NovaError> {
        let existing = self.find_action(task_id, idempotency_key)?;
        Ok(match existing {
            None => NovaError::Config("动作尚未创建计划记录，无法启动".to_string()),
            Some(action) if action.status == ActionStatus::Started => {
                NovaError::Config("该操作已经处于进行中状态".to_string())
            }
            Some(action) if is_read_only => NovaError::Config(format!(
                "只读动作在当前状态（{:?}）下也无法启动",
                action.status
            )),
            Some(action) if action.status == ActionStatus::Succeeded => NovaError::Config(
                "相同 idempotency key 的操作已经成功执行过，拒绝重复执行".to_string(),
            ),
            Some(action) if action.status == ActionStatus::Uncertain => NovaError::Config(
                "该操作状态不确定（可能已部分执行），需要先经过用户确认才能重试".to_string(),
            ),
            Some(_) => {
                NovaError::Config("该非只读操作已有终态记录，重试前需要新的确认".to_string())
            }
        })
    }

    /// Records a terminal outcome for an action, via a single conditional
    /// `UPDATE ... WHERE status = 'started'` — the same read-free
    /// compare-and-set pattern as [`Database::start_action`], so two
    /// concurrent connections racing to finish the same action can never
    /// both succeed. `status` must be one of `succeeded`/`failed`/
    /// `cancelled` — anything else is rejected before touching the
    /// database, since `planned`/`started`/`uncertain` are not valid
    /// finishing states.
    pub fn finish_action(
        &self,
        task_id: &str,
        idempotency_key: &str,
        status: ActionStatus,
        evidence_ref: Option<&str>,
        error_summary: Option<&str>,
    ) -> NovaResult<TaskAction> {
        if !matches!(
            status,
            ActionStatus::Succeeded | ActionStatus::Failed | ActionStatus::Cancelled
        ) {
            return Err(NovaError::Config(
                "finish_action 只能写入 succeeded/failed/cancelled".to_string(),
            ));
        }

        let now = now();
        let rows_affected = self.conn().execute(
            "UPDATE task_actions
             SET status = ?1, finished_at = ?2, evidence_ref = ?3, error_summary = ?4, updated_at = ?2
             WHERE task_id = ?5 AND idempotency_key = ?6 AND status = 'started'",
            params![status.as_str(), now, evidence_ref, error_summary, task_id, idempotency_key],
        )?;

        if rows_affected == 0 {
            return Err(match self.find_action(task_id, idempotency_key)? {
                None => NovaError::Config("动作记录不存在".to_string()),
                Some(_) => NovaError::Config("只有处于 started 状态的动作才能被结束".to_string()),
            });
        }

        self.find_action(task_id, idempotency_key)?
            .ok_or_else(|| NovaError::Database("动作记录在更新后丢失".to_string()))
    }

    /// Explicit transition out of `uncertain` back to `planned` — the only
    /// way an uncertain action becomes retryable again; `start_action`
    /// itself has no path that reaches an uncertain action, so calling
    /// this is a deliberate, separate step calling code must take before a
    /// retry is even possible (never an automatic side effect of
    /// `start_action`/recovery). Bumps `attempt_number` here rather than
    /// in `start_action`, since this is the actual moment a new attempt is
    /// authorized. Who/why is attributable via `task_state_events`/
    /// `recovery_checkpoints`, not a field on this call itself.
    pub fn acknowledge_uncertain_action(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> NovaResult<TaskAction> {
        let now = now();
        let rows_affected = self.conn().execute(
            "UPDATE task_actions SET status = ?1, attempt_number = attempt_number + 1, updated_at = ?2
             WHERE task_id = ?3 AND idempotency_key = ?4 AND status = 'uncertain'",
            params![ActionStatus::Planned.as_str(), now, task_id, idempotency_key],
        )?;
        if rows_affected == 0 {
            return Err(match self.find_action(task_id, idempotency_key)? {
                None => NovaError::Config("动作记录不存在".to_string()),
                Some(_) => NovaError::Config("只有 uncertain 状态的动作可以被确认".to_string()),
            });
        }
        self.find_action(task_id, idempotency_key)?
            .ok_or_else(|| NovaError::Database("动作记录在更新后丢失".to_string()))
    }

    /// Crash-recovery sweep: any action still `started` (never reached a
    /// terminal status) is now `uncertain` — we cannot know whether it
    /// actually finished before the app stopped. Only rows currently
    /// `started` are touched, so running this repeatedly is a no-op after
    /// the first pass (no duplicate transitions, no duplicate events).
    pub fn mark_started_actions_uncertain(&self) -> NovaResult<Vec<TaskAction>> {
        let now = now();
        let ids: Vec<String> = {
            let mut stmt = self
                .conn()
                .prepare("SELECT id FROM task_actions WHERE status = ?1")?;
            let rows = stmt
                .query_map(params![ActionStatus::Started.as_str()], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.conn().execute(
            "UPDATE task_actions SET status = ?1, updated_at = ?2 WHERE status = ?3",
            params![
                ActionStatus::Uncertain.as_str(),
                now,
                ActionStatus::Started.as_str()
            ],
        )?;
        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            result.push(self.action_by_id(&id)?);
        }
        Ok(result)
    }

    pub fn find_action(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> NovaResult<Option<TaskAction>> {
        self.conn()
            .query_row(
                "SELECT id, task_id, action_type, idempotency_key, attempt_number, status, started_at, finished_at, evidence_ref, error_summary, created_at, updated_at
                 FROM task_actions WHERE task_id = ?1 AND idempotency_key = ?2",
                params![task_id, idempotency_key],
                read_action,
            )
            .optional()
            .map_err(Into::into)
    }

    fn action_by_id(&self, id: &str) -> NovaResult<TaskAction> {
        self.conn()
            .query_row(
                "SELECT id, task_id, action_type, idempotency_key, attempt_number, status, started_at, finished_at, evidence_ref, error_summary, created_at, updated_at
                 FROM task_actions WHERE id = ?1",
                params![id],
                read_action,
            )
            .map_err(Into::into)
    }

    pub fn list_actions_for_task(&self, task_id: &str) -> NovaResult<Vec<TaskAction>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, task_id, action_type, idempotency_key, attempt_number, status, started_at, finished_at, evidence_ref, error_summary, created_at, updated_at
             FROM task_actions WHERE task_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id], read_action)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn is_unique_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(inner, _) if inner.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn read_action(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskAction> {
    let status: String = row.get(5)?;
    Ok(TaskAction {
        id: row.get(0)?,
        task_id: row.get(1)?,
        action_type: row.get(2)?,
        idempotency_key: row.get(3)?,
        attempt_number: row.get(4)?,
        status: parse_enum(status)?,
        started_at: row.get(6)?,
        finished_at: row.get(7)?,
        evidence_ref: row.get(8)?,
        error_summary: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
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
            "nova-actions-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db_with_task(dir: &std::path::Path) -> (Database, String) {
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
        (db, task.id)
    }

    #[test]
    fn planning_the_same_idempotency_key_twice_is_a_no_op() {
        let dir = temp_dir("plan-twice");
        let (db, task_id) = db_with_task(&dir);
        let first = db.plan_action(&task_id, "write_file", "key-1").unwrap();
        let second = db.plan_action(&task_id, "write_file", "key-1").unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.attempt_number, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_read_only_action_cannot_succeed_twice_under_the_same_key() {
        let dir = temp_dir("no-double-success");
        let (db, task_id) = db_with_task(&dir);
        db.plan_action(&task_id, "git_commit", "key-1").unwrap();
        db.start_action(&task_id, "key-1", false).unwrap();
        db.finish_action(
            &task_id,
            "key-1",
            ActionStatus::Succeeded,
            Some("sha:abc"),
            None,
        )
        .unwrap();

        let retry = db.start_action(&task_id, "key-1", false);
        assert!(
            retry.is_err(),
            "a destructive action that already succeeded must never restart under the same key"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_started_action_becomes_uncertain_after_a_simulated_crash() {
        let dir = temp_dir("started-to-uncertain");
        let (db, task_id) = db_with_task(&dir);
        db.plan_action(&task_id, "install_dependency", "key-1")
            .unwrap();
        db.start_action(&task_id, "key-1", false).unwrap();

        let swept = db.mark_started_actions_uncertain().unwrap();
        assert_eq!(swept.len(), 1);
        let action = db.find_action(&task_id, "key-1").unwrap().unwrap();
        assert_eq!(action.status, ActionStatus::Uncertain);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uncertain_actions_are_never_auto_retried() {
        let dir = temp_dir("uncertain-no-auto-retry");
        let (db, task_id) = db_with_task(&dir);
        db.plan_action(&task_id, "install_dependency", "key-1")
            .unwrap();
        db.start_action(&task_id, "key-1", false).unwrap();
        db.mark_started_actions_uncertain().unwrap();

        let retry = db.start_action(&task_id, "key-1", false);
        assert!(
            retry.is_err(),
            "an uncertain non-read-only action must not be startable again without acknowledgement"
        );

        db.acknowledge_uncertain_action(&task_id, "key-1").unwrap();
        let after_ack = db.start_action(&task_id, "key-1", false).unwrap();
        assert_eq!(after_ack.status, ActionStatus::Started);
        assert_eq!(
            after_ack.attempt_number, 2,
            "acknowledged retry is a new attempt"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_actions_may_be_retried_freely_even_after_success() {
        let dir = temp_dir("read-only-retry");
        let (db, task_id) = db_with_task(&dir);
        db.plan_action(&task_id, "list_files", "key-1").unwrap();
        db.start_action(&task_id, "key-1", true).unwrap();
        db.finish_action(&task_id, "key-1", ActionStatus::Succeeded, None, None)
            .unwrap();

        let retried = db.start_action(&task_id, "key-1", true).unwrap();
        assert_eq!(retried.status, ActionStatus::Started);
        assert_eq!(retried.attempt_number, 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn multiple_recovery_sweeps_do_not_repeat_or_duplicate() {
        let dir = temp_dir("multi-sweep");
        let (db, task_id) = db_with_task(&dir);
        db.plan_action(&task_id, "install_dependency", "key-1")
            .unwrap();
        db.start_action(&task_id, "key-1", false).unwrap();

        let first_sweep = db.mark_started_actions_uncertain().unwrap();
        let second_sweep = db.mark_started_actions_uncertain().unwrap();
        let third_sweep = db.mark_started_actions_uncertain().unwrap();

        assert_eq!(first_sweep.len(), 1);
        assert_eq!(
            second_sweep.len(),
            0,
            "an already-uncertain action must not be swept again"
        );
        assert_eq!(third_sweep.len(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_actions_for_task_returns_every_planned_action_in_order() {
        let dir = temp_dir("list-actions");
        let (db, task_id) = db_with_task(&dir);
        db.plan_action(&task_id, "write_file", "key-1").unwrap();
        db.plan_action(&task_id, "git_commit", "key-2").unwrap();
        let actions = db.list_actions_for_task(&task_id).unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].idempotency_key, "key-1");
        assert_eq!(actions[1].idempotency_key, "key-2");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finish_action_rejects_a_non_terminal_target_status() {
        let dir = temp_dir("finish-rejects-non-terminal");
        let (db, task_id) = db_with_task(&dir);
        db.plan_action(&task_id, "write_file", "key-1").unwrap();
        db.start_action(&task_id, "key-1", false).unwrap();
        let result = db.finish_action(&task_id, "key-1", ActionStatus::Started, None, None);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
