use rusqlite::params;

use crate::db::Database;
use crate::error::NovaResult;
use crate::models::{Actor, ProjectDirectoryState, RecoveryCheckpoint, RecoverySummary, TaskState};

use super::projects::check_project_directory_state;
use super::{new_id, now};

fn state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Draft => "起草",
        TaskState::AwaitingConfirmation => "等待确认",
        TaskState::Confirmed => "已确认",
        TaskState::Queued => "排队中",
        TaskState::Running => "执行中",
        TaskState::Verifying => "验证中",
        TaskState::SafeFixing => "安全修复中",
        TaskState::PreviewReady => "预览就绪",
        TaskState::AwaitingUserAcceptance => "等待用户确认",
        TaskState::Completed => "已完成",
        TaskState::NeedsUserAction => "需要用户处理",
        TaskState::Interrupted => "已中断",
        TaskState::Failed => "失败",
        TaskState::Cancelled => "已取消",
    }
}

impl Database {
    fn latest_recovery_checkpoint_for_task(
        &self,
        task_id: &str,
    ) -> NovaResult<Option<RecoveryCheckpoint>> {
        self.conn()
            .query_row(
                "SELECT id, project_id, task_id, detected_state, summary, risk_flag, resolved_at, resolution, created_at
                 FROM recovery_checkpoints WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
                params![task_id],
                read_checkpoint,
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other.into()),
            })
    }

    /// Runs on every app startup, before any command execution or AI call
    /// is allowed to happen. Steps, matching the Phase 2 spec:
    ///
    /// 1. Sweep any `task_actions` row still `started` into `uncertain`
    ///    (never auto-retried — see `storage::actions`).
    /// 2. Find the last-active project; if none, there is nothing to
    ///    recover.
    /// 3. If that project's directory is missing/moved, report that
    ///    without touching any task state — Nova does not guess where the
    ///    directory went.
    /// 4. Find the project's current unfinished task, if any.
    /// 5. If it was in a state where a real action could have been in
    ///    flight (`running`/`verifying`/`safe_fixing`), transition it to
    ///    `interrupted` (via the normal, audited `transition_task` path)
    ///    and record a `recovery_checkpoints` row explaining what happened.
    ///    Nothing here ever re-runs a command, re-invokes an AI, or repeats
    ///    a file write — this function only ever reads state and, at most,
    ///    writes a state transition + checkpoint row marking work as
    ///    stopped.
    /// 6. Build a plain-language summary for the minimal resume UI.
    ///
    /// Idempotent by construction: step 5 only fires when the task is
    /// still in an in-flight state, so a second call (e.g. two restarts in
    /// a row without the user acting) finds the task already `interrupted`
    /// and reuses the existing checkpoint instead of writing a new one.
    pub fn run_startup_recovery(&self) -> NovaResult<Option<RecoverySummary>> {
        self.mark_started_actions_uncertain()?;

        let Some(project) = self.last_active_project()? else {
            return Ok(None);
        };

        let directory_state = check_project_directory_state(&project);
        let task = self.unfinished_task_for_project(&project.id)?;

        if directory_state != ProjectDirectoryState::Ok {
            return Ok(Some(RecoverySummary {
                project_id: project.id.clone(),
                project_name: project.name.clone(),
                task,
                last_stable_state: None,
                interrupted_step: None,
                has_risk: true,
                message: format!(
                    "上次打开的项目“{}”所在目录当前不可用（可能已被移动或删除），请先确认目录位置后再继续。",
                    project.name
                ),
                technical_detail: Some(format!("normalized_path={}", project.normalized_path)),
            }));
        }

        let Some(task) = task else {
            return Ok(None);
        };

        let (checkpoint_state, checkpoint_summary, risk_flag) = if task
            .state
            .is_potentially_in_flight()
        {
            let from_state = task.state;
            self.transition_task(
                &task.id,
                TaskState::Interrupted,
                Actor::System,
                Some("应用重启，检测到未完成的操作"),
                None,
                true,
                Some("自动从运行中状态恢复为已中断，未重新执行任何命令"),
            )?;

            let uncertain_count: i64 = self.conn().query_row(
                "SELECT COUNT(*) FROM task_actions WHERE task_id = ?1 AND status = 'uncertain'",
                params![task.id],
                |row| row.get(0),
            )?;
            let risk = uncertain_count > 0;
            let summary = format!(
                "上次工作在“{}”阶段被中断，Nova 没有自动重新执行任何操作。",
                state_label(from_state)
            );
            self.conn().execute(
                "INSERT INTO recovery_checkpoints (id, project_id, task_id, detected_state, summary, risk_flag, resolved_at, resolution, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7)",
                params![new_id(), project.id, task.id, from_state.as_str(), summary, risk as i64, now()],
            )?;
            (from_state, summary, risk)
        } else if task.state == TaskState::Interrupted {
            match self.latest_recovery_checkpoint_for_task(&task.id)? {
                Some(existing) => {
                    let state: TaskState = existing
                        .detected_state
                        .parse()
                        .unwrap_or(TaskState::Interrupted);
                    (state, existing.summary, existing.risk_flag)
                }
                None => (
                    task.state,
                    format!(
                        "上次工作被中断，当前处于“{}”状态。",
                        state_label(task.state)
                    ),
                    false,
                ),
            }
        } else {
            (
                task.state,
                format!(
                    "上次工作停留在“{}”阶段，尚未完成。",
                    state_label(task.state)
                ),
                false,
            )
        };

        let refreshed_task = self.task(&task.id)?;

        Ok(Some(RecoverySummary {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            task: Some(refreshed_task),
            last_stable_state: Some(checkpoint_state.as_str().to_string()),
            interrupted_step: Some(state_label(checkpoint_state).to_string()),
            has_risk: risk_flag,
            message: checkpoint_summary,
            technical_detail: None,
        }))
    }

    /// Marks a recovery checkpoint as acted on ("继续"/"查看当前结果"/"取消
    /// 本任务") — purely a record of the user's choice, it does not itself
    /// perform the chosen action.
    pub fn resolve_recovery_checkpoint(
        &self,
        checkpoint_id: &str,
        resolution: &str,
    ) -> NovaResult<()> {
        self.conn().execute(
            "UPDATE recovery_checkpoints SET resolved_at = ?1, resolution = ?2 WHERE id = ?3",
            params![now(), resolution, checkpoint_id],
        )?;
        Ok(())
    }
}

fn read_checkpoint(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecoveryCheckpoint> {
    let risk_flag: i64 = row.get(5)?;
    Ok(RecoveryCheckpoint {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        detected_state: row.get(3)?,
        summary: row.get(4)?,
        risk_flag: risk_flag != 0,
        resolved_at: row.get(6)?,
        resolution: row.get(7)?,
        created_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CreateProjectRequest, CreateTaskRequest};
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-recovery-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db_with_active_project(dir: &std::path::Path) -> (Database, String) {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let project = db
            .create_project(&CreateProjectRequest {
                name: "Demo".to_string(),
                local_path: dir.to_string_lossy().into_owned(),
                source_type: "local".to_string(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap();
        db.set_last_active_project(&project.id).unwrap();
        (db, project.id)
    }

    fn task_at(db: &Database, project_id: &str, target: TaskState) -> String {
        let task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project_id.to_string(),
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
        if target == TaskState::Draft {
            return task.id;
        }
        let path = [
            TaskState::AwaitingConfirmation,
            TaskState::Confirmed,
            TaskState::Queued,
            TaskState::Running,
            TaskState::Verifying,
            TaskState::SafeFixing,
        ];
        for state in path {
            db.transition_task(&task.id, state, Actor::System, None, None, false, None)
                .unwrap();
            if state == target {
                return task.id;
            }
        }
        task.id
    }

    #[test]
    fn no_last_active_project_means_nothing_to_recover() {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let summary = db.run_startup_recovery().unwrap();
        assert!(summary.is_none());
    }

    #[test]
    fn each_in_flight_state_is_recovered_to_interrupted() {
        for target in [
            TaskState::Running,
            TaskState::Verifying,
            TaskState::SafeFixing,
        ] {
            let dir = temp_dir(&format!("in-flight-{target}"));
            let (db, project_id) = db_with_active_project(&dir);
            let task_id = task_at(&db, &project_id, target);

            let summary = db
                .run_startup_recovery()
                .unwrap()
                .expect("expected a recovery summary");
            assert_eq!(summary.project_id, project_id);
            assert!(
                summary.message.contains("中断") || summary.message.contains("没有自动重新执行")
            );

            let task = db.task(&task_id).unwrap();
            assert_eq!(task.state, TaskState::Interrupted);
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn repeated_recovery_scans_are_idempotent() {
        let dir = temp_dir("repeated-scan");
        let (db, project_id) = db_with_active_project(&dir);
        let task_id = task_at(&db, &project_id, TaskState::Running);

        db.run_startup_recovery().unwrap();
        let events_after_first = db.list_task_state_events(&task_id).unwrap().len();
        let checkpoints_after_first: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM recovery_checkpoints WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .unwrap();

        db.run_startup_recovery().unwrap();
        db.run_startup_recovery().unwrap();

        let events_after_more = db.list_task_state_events(&task_id).unwrap().len();
        let checkpoints_after_more: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM recovery_checkpoints WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            events_after_first, events_after_more,
            "no duplicate state-event should be written on repeat scans"
        );
        assert_eq!(
            checkpoints_after_first, checkpoints_after_more,
            "no duplicate checkpoint should be written on repeat scans"
        );
        assert_eq!(checkpoints_after_first, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_never_re_executes_a_destructive_action() {
        let dir = temp_dir("no-re-execute");
        let (db, project_id) = db_with_active_project(&dir);
        let task_id = task_at(&db, &project_id, TaskState::Running);
        db.plan_action(&task_id, "git_commit", "commit-key-1")
            .unwrap();
        db.start_action(&task_id, "commit-key-1", false).unwrap();

        db.run_startup_recovery().unwrap();

        let action = db.find_action(&task_id, "commit-key-1").unwrap().unwrap();
        assert_eq!(
            action.status,
            crate::models::ActionStatus::Uncertain,
            "an in-flight action must become uncertain, never silently re-succeed"
        );
        // Uncertain actions cannot be started again without explicit
        // acknowledgement — recovery itself never calls start_action.
        assert!(db.start_action(&task_id, "commit-key-1", false).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_active_project_is_surfaced_in_the_summary() {
        let dir = temp_dir("last-active-surfaced");
        let (db, project_id) = db_with_active_project(&dir);
        let _task_id = task_at(&db, &project_id, TaskState::Draft);

        let summary = db.run_startup_recovery().unwrap().unwrap();
        assert_eq!(summary.project_id, project_id);
        assert_eq!(summary.project_name, "Demo");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_project_directory_produces_a_safe_risk_flagged_summary() {
        let dir = temp_dir("missing-dir-recovery");
        let (db, project_id) = db_with_active_project(&dir);
        let _task_id = task_at(&db, &project_id, TaskState::Draft);
        fs::remove_dir_all(&dir).unwrap();

        let summary = db.run_startup_recovery().unwrap().unwrap();
        assert!(summary.has_risk);
        assert!(summary.message.contains("不可用"));
    }

    #[test]
    fn recovery_summary_message_matches_the_actual_interrupted_phase() {
        let dir = temp_dir("accurate-summary");
        let (db, project_id) = db_with_active_project(&dir);
        let _task_id = task_at(&db, &project_id, TaskState::Verifying);

        let summary = db.run_startup_recovery().unwrap().unwrap();
        assert_eq!(summary.interrupted_step.as_deref(), Some("验证中"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_in_flight_unfinished_task_still_produces_a_resume_summary() {
        let dir = temp_dir("draft-resume");
        let (db, project_id) = db_with_active_project(&dir);
        let _task_id = task_at(&db, &project_id, TaskState::Draft);

        let summary = db.run_startup_recovery().unwrap().unwrap();
        assert!(!summary.has_risk);
        assert!(summary.message.contains("起草"));
        let _ = fs::remove_dir_all(&dir);
    }
}
