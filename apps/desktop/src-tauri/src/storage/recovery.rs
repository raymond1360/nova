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
    /// is allowed to happen. Steps, matching the Phase 2 spec (remediated:
    /// the repair scan now covers every project, not just the last-active
    /// one — see [`Database::interrupt_all_in_flight_tasks`]):
    ///
    /// 1. Sweep any `task_actions` row still `started` into `uncertain`
    ///    (never auto-retried — see `storage::actions`).
    /// 2. Scan **every project's** tasks for `running`/`verifying`/
    ///    `safe_fixing` and transition each to `interrupted`, recording a
    ///    `recovery_checkpoints` row per task. This repair step does not
    ///    depend on, or get limited by, which project was last active, and
    ///    it runs even if the last-active project's own directory is
    ///    missing. Nothing here ever re-runs a command, re-invokes an AI,
    ///    or repeats a file write — only a state transition + checkpoint
    ///    row marking work as stopped.
    /// 3. Separately, find the last-active project purely to build a
    ///    plain-language summary for the minimal resume UI — if its
    ///    directory is missing/moved, the summary flags that risk without
    ///    skipping step 2 (which has already run for every project,
    ///    including this one, by this point).
    ///
    /// Idempotent by construction: step 2's query only ever selects tasks
    /// still in an in-flight state, so a second call (e.g. two restarts in
    /// a row without the user acting) finds nothing left to transition and
    /// writes no new checkpoints.
    pub fn run_startup_recovery(&self) -> NovaResult<Option<RecoverySummary>> {
        self.mark_started_actions_uncertain()?;
        self.interrupt_all_in_flight_tasks()?;

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

        // By this point step 2 has already interrupted `task` if it was
        // in-flight, so this only ever reads state — it never transitions
        // anything itself.
        let (checkpoint_state, checkpoint_summary, risk_flag) =
            if task.state == TaskState::Interrupted {
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

        Ok(Some(RecoverySummary {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
            task: Some(task),
            last_stable_state: Some(checkpoint_state.as_str().to_string()),
            interrupted_step: Some(state_label(checkpoint_state).to_string()),
            has_risk: risk_flag,
            message: checkpoint_summary,
            technical_detail: None,
        }))
    }

    /// Scans **all** projects' tasks for `running`/`verifying`/
    /// `safe_fixing` and transitions each to `interrupted`, independent of
    /// which project (if any) is last-active. This is the actual data-repair
    /// half of startup recovery; [`Database::run_startup_recovery`]'s
    /// last-active-project logic is purely about what to show the user, not
    /// which tasks get fixed.
    ///
    /// Each transition goes through the normal, CAS-guarded
    /// `transition_task` path, so two connections (e.g. two Nova processes)
    /// running this concurrently can never both "win" on the same task —
    /// the loser's `transition_task` call simply returns `Err` (state no
    /// longer matches) and is skipped here, not retried or treated as a
    /// hard failure of the whole sweep.
    ///
    /// The `transition_task` call and the following `recovery_checkpoints`
    /// insert are two separate statements, not one transaction: if the
    /// process were to crash between them, the task is already durably
    /// `interrupted` (the important, safety-relevant fact), and the only
    /// consequence of a missing checkpoint row is that
    /// `run_startup_recovery`'s summary falls back to a generic "was
    /// interrupted" message (see its `None =>` branch) instead of the
    /// richer one — a graceful degradation, not a correctness gap, and it
    /// still never writes a duplicate checkpoint on the next scan.
    fn interrupt_all_in_flight_tasks(&self) -> NovaResult<()> {
        let in_flight: Vec<(String, String, TaskState)> = {
            let mut stmt = self.conn().prepare(
                "SELECT id, project_id, state FROM tasks WHERE state IN ('running', 'verifying', 'safe_fixing')",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    let state: String = row.get(2)?;
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, state))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|(id, project_id, state)| {
                    (
                        id,
                        project_id,
                        state.parse::<TaskState>().unwrap_or(TaskState::Interrupted),
                    )
                })
                // Redundant with the SQL `WHERE state IN (...)` above by
                // design: if the SQL literal list and this Rust-side
                // definition of "in-flight" were ever to drift apart, this
                // keeps the actual behavior anchored to the single
                // authoritative definition in `TaskState`.
                .filter(|(_, _, state)| state.is_potentially_in_flight())
                .collect()
        };

        for (task_id, project_id, from_state) in in_flight {
            let transitioned = self.transition_task(
                &task_id,
                TaskState::Interrupted,
                Actor::System,
                Some("应用重启，检测到未完成的操作"),
                None,
                true,
                Some("自动从运行中状态恢复为已中断，未重新执行任何命令"),
            );
            if transitioned.is_err() {
                // Someone/something else (another connection, or this task
                // simply isn't in `from_state` anymore) already handled
                // it — correct to skip, not an error for the whole sweep.
                continue;
            }

            let uncertain_count: i64 = self.conn().query_row(
                "SELECT COUNT(*) FROM task_actions WHERE task_id = ?1 AND status = 'uncertain'",
                params![task_id],
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
                params![new_id(), project_id, task_id, from_state.as_str(), summary, risk as i64, now()],
            )?;
        }

        Ok(())
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

    /// Registers a second project (sharing `db`'s connection) without
    /// making it last-active, and drives a task inside it to `target`.
    fn second_project_task(
        db: &Database,
        dir: &std::path::Path,
        target: TaskState,
    ) -> (String, String) {
        let project = db
            .create_project(&crate::models::CreateProjectRequest {
                name: "Second".to_string(),
                local_path: dir.to_string_lossy().into_owned(),
                source_type: "local".to_string(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap();
        let task_id = task_at(db, &project.id, target);
        (project.id, task_id)
    }

    #[test]
    fn a_running_task_in_a_non_last_active_project_is_still_interrupted() {
        let dir_active = temp_dir("multi-project-active");
        let dir_other = temp_dir("multi-project-other");
        let (db, active_project_id) = db_with_active_project(&dir_active);
        // The last-active project itself has nothing in flight.
        let _active_task = task_at(&db, &active_project_id, TaskState::Draft);
        let (_other_project_id, other_task_id) =
            second_project_task(&db, &dir_other, TaskState::Running);

        db.run_startup_recovery().unwrap();

        let other_task = db.task(&other_task_id).unwrap();
        assert_eq!(
            other_task.state,
            TaskState::Interrupted,
            "a running task must be interrupted even though its project was never last-active"
        );
        let _ = fs::remove_dir_all(&dir_active);
        let _ = fs::remove_dir_all(&dir_other);
    }

    #[test]
    fn in_flight_tasks_across_more_than_two_projects_are_all_recovered() {
        let dir_active = temp_dir("multi-project-3-active");
        let dir_b = temp_dir("multi-project-3-b");
        let dir_c = temp_dir("multi-project-3-c");
        let (db, active_project_id) = db_with_active_project(&dir_active);
        let active_task_id = task_at(&db, &active_project_id, TaskState::Verifying);
        let (_project_b, task_b) = second_project_task(&db, &dir_b, TaskState::Running);
        let (_project_c, task_c) = second_project_task(&db, &dir_c, TaskState::SafeFixing);

        db.run_startup_recovery().unwrap();

        for task_id in [&active_task_id, &task_b, &task_c] {
            let task = db.task(task_id).unwrap();
            assert_eq!(
                task.state,
                TaskState::Interrupted,
                "task {task_id} must be interrupted"
            );
        }

        let _ = fs::remove_dir_all(&dir_active);
        let _ = fs::remove_dir_all(&dir_b);
        let _ = fs::remove_dir_all(&dir_c);
    }

    #[test]
    fn multiple_in_flight_tasks_within_a_single_project_are_all_recovered() {
        let dir = temp_dir("multi-task-one-project");
        let (db, project_id) = db_with_active_project(&dir);
        // Two independent tasks in the same project, both in flight.
        let task_a = task_at(&db, &project_id, TaskState::Running);
        let task_b = {
            let task = db
                .create_task(
                    &CreateTaskRequest {
                        project_id: project_id.clone(),
                        title: "second task".to_string(),
                        goal: None,
                        in_scope: None,
                        not_in_scope: None,
                        done_criteria: None,
                        ai_kind: None,
                    },
                    Actor::User,
                )
                .unwrap();
            for state in [
                TaskState::AwaitingConfirmation,
                TaskState::Confirmed,
                TaskState::Queued,
                TaskState::Running,
                TaskState::Verifying,
            ] {
                db.transition_task(&task.id, state, Actor::System, None, None, false, None)
                    .unwrap();
            }
            task.id
        };

        db.run_startup_recovery().unwrap();

        assert_eq!(db.task(&task_a).unwrap().state, TaskState::Interrupted);
        assert_eq!(db.task(&task_b).unwrap().state, TaskState::Interrupted);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_last_active_directory_does_not_block_the_database_repair_sweep() {
        let dir_active = temp_dir("missing-dir-still-repairs-active");
        let dir_other = temp_dir("missing-dir-still-repairs-other");
        let (db, active_project_id) = db_with_active_project(&dir_active);
        let active_task_id = task_at(&db, &active_project_id, TaskState::Running);
        let (_other_project_id, other_task_id) =
            second_project_task(&db, &dir_other, TaskState::Verifying);

        fs::remove_dir_all(&dir_active).unwrap();

        let summary = db.run_startup_recovery().unwrap().unwrap();
        assert!(summary.has_risk, "missing directory must still be flagged");

        // Both the last-active project's own task AND the other project's
        // task must still have been repaired at the database level, even
        // though the last-active directory itself is gone.
        assert_eq!(
            db.task(&active_task_id).unwrap().state,
            TaskState::Interrupted
        );
        assert_eq!(
            db.task(&other_task_id).unwrap().state,
            TaskState::Interrupted
        );

        let _ = fs::remove_dir_all(&dir_other);
    }
}
