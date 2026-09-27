//! Phase 4 (evidence and document gate): a single, read-only,
//! backend-authoritative aggregation of everything Phase 3 already records
//! for one task — a real timeline, a categorized evidence summary, and the
//! same completion-gate facts `documents::final_result_doc` already
//! computes — for the frontend's trusted evidence view.
//!
//! Deliberately does **not** write anything. In particular, this does not
//! populate the pre-existing (and currently unused-by-production-code)
//! `timeline_events`/`evidence_refs` tables — doing so would mean adding new
//! write call sites throughout the already-reviewed `run_core_loop`
//! orchestration, which this work package's scope explicitly forbids
//! touching without a proven regression. Instead, the timeline below is
//! *derived*, on every call, purely from facts Phase 3 already persists for
//! other reasons: `task_state_events` (every real state transition),
//! `task_actions` (the Action Ledger — dev/test/build steps), `test_
//! evidence`/`build_evidence` (Nova's own independent verification results),
//! `no_tests_confirmations`/`no_build_confirmations` (explicit user
//! confirmations), and `user_acceptance` (explicit user accept/reject
//! decisions). Because nothing here is stored, calling this twice in a row
//! can never produce duplicate events, and the exact same database state —
//! before or after a restart — always derives the exact same timeline.
//!
//! Every value here comes from a real, already-persisted row — this module
//! never fabricates an event for something that has not happened, and never
//! reports `planned`/`started` as `passed`.

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::{NovaError, NovaResult};
use crate::models::{
    ActionStatus, BuildEvidence, CommandEvidence, FileChangeEvidence, GitEvidence, Task, TaskState,
    TestEvidence, UserAcceptance,
};

use super::documents::adopted_passing_generation_round;

/// One real, already-happened fact in a task's history, normalized into a
/// single shape regardless of which underlying table it came from — see
/// the module doc comment for exactly which tables feed this.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimelineEntry {
    /// Stable and deterministic: derived from the real underlying row's own
    /// id (or, for the one row-less entry — task creation — from the task's
    /// own id), so re-deriving the timeline from the same database state
    /// always yields the same ids, never fresh random ones.
    pub id: String,
    pub event_type: String,
    pub title: String,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub result_generation: Option<i64>,
    pub round: Option<i64>,
    pub needs_user_action: bool,
    /// True when `result_generation` is a strictly earlier generation than
    /// the task's *current* one — real history, always shown, but never a
    /// signal this entry may be used to authorize the current result.
    pub is_historical: bool,
    /// Internal-only sort key: `(created_at, tie_break_rank, id)` — never
    /// serialized, exists purely so ties at identical timestamps (the same
    /// database transaction, or a clock resolution coarser than the event
    /// rate) still produce a fully deterministic order.
    #[serde(skip)]
    sort_key: (String, u8, String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestEvidenceEntry {
    #[serde(flatten)]
    pub evidence: TestEvidence,
    pub is_current_generation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildEvidenceEntry {
    #[serde(flatten)]
    pub evidence: BuildEvidence,
    pub is_current_generation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskEvidenceOverview {
    pub project_id: String,
    pub task_id: String,
    pub task_state: TaskState,
    pub result_generation: i64,
    pub pending_round: i64,
    pub needs_user_action: bool,
    /// Whether Nova's own independent test/build verification has a
    /// passing (or legitimately-confirmed-not-applicable) result available
    /// right now — the same fact `documents::final_result_doc` computes,
    /// via the exact same shared helper, never a second hand-written copy.
    pub acceptance_ready: bool,
    /// Whether the task is genuinely `completed` — real user acceptance for
    /// the current generation, not merely "ready to be accepted."
    pub is_completed: bool,
    pub timeline: Vec<TimelineEntry>,
    pub file_changes: Vec<FileChangeEvidence>,
    pub git_events: Vec<GitEvidence>,
    pub commands: Vec<CommandEvidence>,
    pub tests: Vec<TestEvidenceEntry>,
    pub builds: Vec<BuildEvidenceEntry>,
    /// Every acceptance/rejection the task has ever recorded, oldest first
    /// — real audit history. Only the current-generation row (if any, and
    /// only when `accepted`) is what `is_completed` above actually depends
    /// on; older rows here are shown for transparency only.
    pub acceptance_history: Vec<UserAcceptance>,
}

const TIE_TASK_CREATED: u8 = 0;
const TIE_STATE_EVENT: u8 = 1;
const TIE_DEV_ACTION: u8 = 2;
const TIE_TEST_ACTION: u8 = 3;
const TIE_BUILD_ACTION: u8 = 4;
const TIE_NO_TESTS_CONFIRMATION: u8 = 5;
const TIE_NO_BUILD_CONFIRMATION: u8 = 6;
const TIE_USER_ACCEPTANCE: u8 = 7;

fn state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Draft => "草稿",
        TaskState::AwaitingConfirmation => "待确认需求",
        TaskState::Confirmed => "已确认，待开始",
        TaskState::Queued => "排队中",
        TaskState::Running => "开发中",
        TaskState::Verifying => "测试与构建验证中",
        TaskState::SafeFixing => "自动修复中",
        TaskState::PreviewReady => "结果已就绪",
        TaskState::AwaitingUserAcceptance => "等待用户验收",
        TaskState::Completed => "已完成",
        TaskState::NeedsUserAction => "需要用户处理",
        TaskState::Interrupted => "已中断",
        TaskState::Failed => "失败",
        TaskState::Cancelled => "已取消",
    }
}

/// Normalizes a real `test_evidence` row's `status` into the shared,
/// user-facing vocabulary: `passed` / `failed` / `cancelled` / `timed_out`
/// / `insufficient_evidence` / `not_applicable`.
///
/// Phase 4 remediation (Major 2): a `no_tests_applicable` status string is
/// **not**, by itself, authorization — this now re-verifies a real,
/// exact-key `no_tests_confirmations` row exists
/// (`documents::no_tests_applicable_is_authorized`) before ever mapping to
/// `not_applicable`. An orphaned/unconfirmed row (which should never arise
/// from the real production write path, but must never be silently trusted
/// either) fails closed to `insufficient_evidence` — an existing, stable
/// status the frontend already handles, not a new protocol surface.
fn normalize_test_evidence_status(
    db: &Database,
    project_id: &str,
    task_id: &str,
    evidence: &TestEvidence,
) -> NovaResult<String> {
    if evidence.status == "no_tests_applicable" {
        return Ok(
            if super::documents::no_tests_applicable_is_authorized(
                db,
                project_id,
                task_id,
                evidence.result_generation,
                evidence.round,
            )? {
                "not_applicable".to_string()
            } else {
                "insufficient_evidence".to_string()
            },
        );
    }
    Ok(evidence.status.clone())
}

/// The build equivalent of [`normalize_test_evidence_status`] — see
/// `documents::no_build_applicable_is_authorized` for why build's check
/// also accepts Nova's own current mechanical re-derivation, not just a
/// confirmation row.
fn normalize_build_evidence_status(
    db: &Database,
    project_id: &str,
    task_id: &str,
    evidence: &BuildEvidence,
) -> NovaResult<String> {
    if evidence.status == "no_build_applicable" {
        return Ok(
            if super::documents::no_build_applicable_is_authorized(
                db,
                project_id,
                task_id,
                evidence.result_generation,
                evidence.round,
                evidence.authorization_source.as_deref(),
            )? {
                "not_applicable".to_string()
            } else {
                "insufficient_evidence".to_string()
            },
        );
    }
    Ok(evidence.status.clone())
}

/// Builds the trusted, read-only evidence/timeline/completion view for one
/// task. Fails if `project_id` does not own `task_id` — checked first,
/// before any other query runs, so a wrong project id can never observe
/// even a hint of another project's task existing.
pub fn build_task_evidence_overview(
    db: &Database,
    project_id: &str,
    task_id: &str,
) -> NovaResult<TaskEvidenceOverview> {
    let task = db.task(task_id)?;
    if task.project_id != project_id {
        return Err(NovaError::Config(
            "project_id 与任务所属项目不一致，已拒绝读取".to_string(),
        ));
    }

    let pending_round = super::current_pending_round(db, project_id, task_id)?;
    let adopted = adopted_passing_generation_round(db, project_id, task_id, &task)?;
    let acceptance_ready = task.state == TaskState::AwaitingUserAcceptance && adopted.is_some();
    let acceptance_history = db.list_user_acceptance_for_task(project_id, task_id)?;
    let is_completed = task.state == TaskState::Completed
        && acceptance_history
            .iter()
            .any(|a| a.result_generation == task.result_generation && a.accepted);

    let mut timeline = Vec::new();
    push_task_created_entry(&mut timeline, &task);
    push_state_event_entries(db, project_id, task_id, &mut timeline)?;
    push_dev_action_entries(db, task_id, &task, &mut timeline)?;
    for generation in 0..=task.result_generation {
        for round in [1_i64, 2] {
            push_test_entries(
                db,
                project_id,
                task_id,
                &task,
                generation,
                round,
                &mut timeline,
            )?;
            push_build_entries(
                db,
                project_id,
                task_id,
                &task,
                generation,
                round,
                &mut timeline,
            )?;
        }
    }
    push_acceptance_entries(&task, &acceptance_history, &mut timeline);
    timeline.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));

    let tests = collect_test_entries(db, project_id, task_id, &task)?;
    let builds = collect_build_entries(db, project_id, task_id, &task)?;

    Ok(TaskEvidenceOverview {
        project_id: project_id.to_string(),
        task_id: task_id.to_string(),
        task_state: task.state,
        result_generation: task.result_generation,
        pending_round,
        needs_user_action: task.state == TaskState::NeedsUserAction,
        acceptance_ready,
        is_completed,
        timeline,
        file_changes: db.list_file_change_evidence_for_task(task_id)?,
        git_events: db.list_git_evidence_for_task(task_id)?,
        commands: db.list_command_evidence_for_task(task_id)?,
        tests,
        builds,
        acceptance_history,
    })
}

fn push_task_created_entry(timeline: &mut Vec<TimelineEntry>, task: &Task) {
    timeline.push(TimelineEntry {
        id: format!("task-created-{}", task.id),
        event_type: "task_created".to_string(),
        title: "任务已创建".to_string(),
        status: "created".to_string(),
        started_at: Some(task.created_at.clone()),
        finished_at: Some(task.created_at.clone()),
        result_generation: None,
        round: None,
        needs_user_action: false,
        is_historical: false,
        sort_key: (task.created_at.clone(), TIE_TASK_CREATED, task.id.clone()),
    });
}

fn push_state_event_entries(
    db: &Database,
    project_id: &str,
    task_id: &str,
    timeline: &mut Vec<TimelineEntry>,
) -> NovaResult<()> {
    for event in db.list_task_state_events(project_id, task_id)? {
        let needs_user_action = event.new_state == TaskState::NeedsUserAction;
        let title = match event.old_state {
            Some(old) => format!(
                "状态变化：{} → {}",
                state_label(old),
                state_label(event.new_state)
            ),
            None => format!("状态变化：{}", state_label(event.new_state)),
        };
        timeline.push(TimelineEntry {
            id: format!("state-event-{}", event.id),
            event_type: "state_transition".to_string(),
            title,
            status: event.new_state.as_str().to_string(),
            started_at: Some(event.created_at.clone()),
            finished_at: Some(event.created_at.clone()),
            result_generation: None,
            round: None,
            needs_user_action,
            is_historical: false,
            sort_key: (event.created_at.clone(), TIE_STATE_EVENT, event.id.clone()),
        });
    }
    Ok(())
}

fn push_dev_action_entries(
    db: &Database,
    task_id: &str,
    task: &Task,
    timeline: &mut Vec<TimelineEntry>,
) -> NovaResult<()> {
    for round in [1_i64, 2] {
        let key = format!("dev-{task_id}-round-{round}");
        if let Some(action) = db.find_action(task_id, &key)? {
            let title = if round == 1 {
                "第一轮开发".to_string()
            } else {
                "第二轮开发（自动修复）".to_string()
            };
            let needs_user_action =
                action.status == ActionStatus::Failed && task.state == TaskState::NeedsUserAction;
            let sort_time = action
                .finished_at
                .clone()
                .or_else(|| action.started_at.clone())
                .unwrap_or_else(|| action.created_at.clone());
            timeline.push(TimelineEntry {
                id: format!("dev-action-{}", action.id),
                event_type: "dev_action".to_string(),
                title,
                status: action.status.as_str().to_string(),
                started_at: action.started_at.clone(),
                finished_at: action.finished_at.clone(),
                result_generation: None,
                round: Some(round),
                needs_user_action,
                is_historical: false,
                sort_key: (sort_time, TIE_DEV_ACTION, action.id.clone()),
            });
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn push_test_entries(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
    generation: i64,
    round: i64,
    timeline: &mut Vec<TimelineEntry>,
) -> NovaResult<()> {
    let is_historical = generation < task.result_generation;
    let key = super::test_action_key(task_id, generation, round);
    let action = db.find_action(task_id, &key)?;
    let evidence = db.list_test_evidence_for_round(project_id, task_id, generation, round)?;
    if action.is_none() && evidence.is_empty() {
        return Ok(());
    }
    let title = format!("第{round}轮独立测试（第 {generation} 代）");
    if let Some(latest) = evidence.last() {
        let status = normalize_test_evidence_status(db, project_id, task_id, latest)?;
        timeline.push(TimelineEntry {
            id: format!("test-evidence-{}", latest.id),
            event_type: "test_evidence".to_string(),
            title,
            status,
            started_at: Some(latest.created_at.clone()),
            finished_at: Some(latest.created_at.clone()),
            result_generation: Some(generation),
            round: Some(round),
            needs_user_action: false,
            is_historical,
            sort_key: (
                latest.created_at.clone(),
                TIE_TEST_ACTION,
                latest.id.clone(),
            ),
        });
    } else if let Some(action) = action {
        let needs_user_action =
            action.status == ActionStatus::Failed && task.state == TaskState::NeedsUserAction;
        let status = match action.status {
            ActionStatus::Planned => "not_run".to_string(),
            other => other.as_str().to_string(),
        };
        let sort_time = action
            .finished_at
            .clone()
            .or_else(|| action.started_at.clone())
            .unwrap_or_else(|| action.created_at.clone());
        timeline.push(TimelineEntry {
            id: format!("test-action-{}", action.id),
            event_type: "test_action".to_string(),
            title,
            status,
            started_at: action.started_at.clone(),
            finished_at: action.finished_at.clone(),
            result_generation: Some(generation),
            round: Some(round),
            needs_user_action,
            is_historical,
            sort_key: (sort_time, TIE_TEST_ACTION, action.id.clone()),
        });
    }
    if let Some(confirmation) =
        db.find_no_tests_confirmation(project_id, task_id, generation, round)?
    {
        timeline.push(TimelineEntry {
            id: format!("no-tests-confirmation-{}", confirmation.id),
            event_type: "no_tests_confirmed".to_string(),
            title: "用户确认：当前无适用测试".to_string(),
            status: "confirmed".to_string(),
            started_at: Some(confirmation.confirmed_at.clone()),
            finished_at: Some(confirmation.confirmed_at.clone()),
            result_generation: Some(generation),
            round: Some(round),
            needs_user_action: false,
            is_historical,
            sort_key: (
                confirmation.confirmed_at.clone(),
                TIE_NO_TESTS_CONFIRMATION,
                confirmation.id.clone(),
            ),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn push_build_entries(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
    generation: i64,
    round: i64,
    timeline: &mut Vec<TimelineEntry>,
) -> NovaResult<()> {
    let is_historical = generation < task.result_generation;
    let key = super::build_action_key(task_id, generation, round);
    let action = db.find_action(task_id, &key)?;
    let evidence = db.list_build_evidence_for_round(project_id, task_id, generation, round)?;
    if action.is_none() && evidence.is_empty() {
        return Ok(());
    }
    let title = format!("第{round}轮独立构建（第 {generation} 代）");
    if let Some(latest) = evidence.last() {
        let status = normalize_build_evidence_status(db, project_id, task_id, latest)?;
        timeline.push(TimelineEntry {
            id: format!("build-evidence-{}", latest.id),
            event_type: "build_evidence".to_string(),
            title,
            status,
            started_at: Some(latest.created_at.clone()),
            finished_at: Some(latest.created_at.clone()),
            result_generation: Some(generation),
            round: Some(round),
            needs_user_action: false,
            is_historical,
            sort_key: (
                latest.created_at.clone(),
                TIE_BUILD_ACTION,
                latest.id.clone(),
            ),
        });
    } else if let Some(action) = action {
        let needs_user_action =
            action.status == ActionStatus::Failed && task.state == TaskState::NeedsUserAction;
        let status = match action.status {
            ActionStatus::Planned => "not_run".to_string(),
            other => other.as_str().to_string(),
        };
        let sort_time = action
            .finished_at
            .clone()
            .or_else(|| action.started_at.clone())
            .unwrap_or_else(|| action.created_at.clone());
        timeline.push(TimelineEntry {
            id: format!("build-action-{}", action.id),
            event_type: "build_action".to_string(),
            title,
            status,
            started_at: action.started_at.clone(),
            finished_at: action.finished_at.clone(),
            result_generation: Some(generation),
            round: Some(round),
            needs_user_action,
            is_historical,
            sort_key: (sort_time, TIE_BUILD_ACTION, action.id.clone()),
        });
    }
    if let Some(confirmation) =
        db.find_no_build_confirmation(project_id, task_id, generation, round)?
    {
        timeline.push(TimelineEntry {
            id: format!("no-build-confirmation-{}", confirmation.id),
            event_type: "no_build_confirmed".to_string(),
            title: "用户确认：当前无需构建".to_string(),
            status: "confirmed".to_string(),
            started_at: Some(confirmation.confirmed_at.clone()),
            finished_at: Some(confirmation.confirmed_at.clone()),
            result_generation: Some(generation),
            round: Some(round),
            needs_user_action: false,
            is_historical,
            sort_key: (
                confirmation.confirmed_at.clone(),
                TIE_NO_BUILD_CONFIRMATION,
                confirmation.id.clone(),
            ),
        });
    }
    Ok(())
}

fn push_acceptance_entries(
    task: &Task,
    acceptance_history: &[UserAcceptance],
    timeline: &mut Vec<TimelineEntry>,
) {
    for acceptance in acceptance_history {
        let is_historical = acceptance.result_generation < task.result_generation;
        let title = if acceptance.accepted {
            "用户已验收结果".to_string()
        } else {
            "用户未通过验收".to_string()
        };
        timeline.push(TimelineEntry {
            id: format!("user-acceptance-{}", acceptance.id),
            event_type: "user_acceptance".to_string(),
            title,
            status: if acceptance.accepted {
                "accepted"
            } else {
                "rejected"
            }
            .to_string(),
            started_at: Some(acceptance.accepted_at.clone()),
            finished_at: Some(acceptance.accepted_at.clone()),
            result_generation: Some(acceptance.result_generation),
            round: None,
            needs_user_action: false,
            is_historical,
            sort_key: (
                acceptance.accepted_at.clone(),
                TIE_USER_ACCEPTANCE,
                acceptance.id.clone(),
            ),
        });
    }
}

fn collect_test_entries(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
) -> NovaResult<Vec<TestEvidenceEntry>> {
    let mut out = Vec::new();
    for generation in 0..=task.result_generation {
        for round in [1_i64, 2] {
            for evidence in
                db.list_test_evidence_for_round(project_id, task_id, generation, round)?
            {
                let is_current_generation = evidence.result_generation == task.result_generation;
                out.push(TestEvidenceEntry {
                    evidence,
                    is_current_generation,
                });
            }
        }
    }
    Ok(out)
}

fn collect_build_entries(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
) -> NovaResult<Vec<BuildEvidenceEntry>> {
    let mut out = Vec::new();
    for generation in 0..=task.result_generation {
        for round in [1_i64, 2] {
            for evidence in
                db.list_build_evidence_for_round(project_id, task_id, generation, round)?
            {
                let is_current_generation = evidence.result_generation == task.result_generation;
                out.push(BuildEvidenceEntry {
                    evidence,
                    is_current_generation,
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Actor, CreateProjectRequest, CreateTaskRequest};
    use crate::storage::evidence::{RecordBuildEvidenceRequest, RecordTestEvidenceRequest};
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-evidence-overview-test-{name}-{}-{}",
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

    #[test]
    fn overview_rejects_a_project_id_that_does_not_own_the_task() {
        let dir_a = temp_dir("wrong-project-a");
        let dir_b = temp_dir("wrong-project-b");
        let (db, _project_a, task_a) = db_with_task(&dir_a, "A");
        let (_db_unused, project_b, _task_b) = db_with_task(&dir_b, "B");
        let result = build_task_evidence_overview(&db, &project_b, &task_a);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn overview_never_leaks_a_sibling_tasks_evidence_within_the_same_project() {
        let dir = temp_dir("sibling-tasks");
        let (db, project_id, task_a) = db_with_task(&dir, "P");
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
            .unwrap()
            .id;
        db.record_test_evidence(&RecordTestEvidenceRequest {
            project_id: &project_id,
            task_id: &task_b,
            result_generation: 0,
            round: 1,
            test_command_redacted: Some("cargo test"),
            status: "passed",
            passed_count: 1,
            failed_count: 0,
            skipped_count: 0,
            exit_code: Some(0),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();

        let overview_a = build_task_evidence_overview(&db, &project_id, &task_a).unwrap();
        assert!(overview_a.tests.is_empty());
        assert!(overview_a
            .timeline
            .iter()
            .all(|entry| !entry.id.contains(&task_b)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timeline_includes_task_created_and_is_stably_ordered_across_repeated_calls() {
        let dir = temp_dir("stable-order");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        db.transition_task(
            &task_id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let first = build_task_evidence_overview(&db, &project_id, &task_id).unwrap();
        let second = build_task_evidence_overview(&db, &project_id, &task_id).unwrap();
        assert_eq!(
            first.timeline, second.timeline,
            "repeated queries must never reorder or duplicate events"
        );
        assert_eq!(first.timeline[0].event_type, "task_created");
        assert!(
            first
                .timeline
                .iter()
                .filter(|e| e.event_type == "state_transition")
                .count()
                >= 1,
            "task creation plus at least the explicit transition must be present"
        );
    }

    #[test]
    fn timeline_never_reports_a_planned_or_started_test_action_as_passed() {
        let dir = temp_dir("no-fake-pass");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        let key = super::super::test_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "test_execution", &key).unwrap();

        let overview = build_task_evidence_overview(&db, &project_id, &task_id).unwrap();
        let entry = overview
            .timeline
            .iter()
            .find(|e| e.event_type == "test_action")
            .expect("a planned test action must still be reported, just never as passed");
        assert_eq!(entry.status, "not_run");
        assert_ne!(entry.status, "passed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_generation_test_evidence_is_marked_historical_and_new_generation_is_not() {
        let dir = temp_dir("historical-marking");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        db.record_test_evidence(&RecordTestEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            test_command_redacted: Some("cargo test"),
            status: "passed",
            passed_count: 1,
            failed_count: 0,
            skipped_count: 0,
            exit_code: Some(0),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        // Bump the real task to generation 1 the same way a genuine
        // successful round does — reaching `preview_ready` for the first
        // time always bumps `result_generation` from 0 to 1.
        drive_to_awaiting_acceptance(&db, &task_id);
        db.record_test_evidence(&RecordTestEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 1,
            round: 1,
            test_command_redacted: Some("cargo test"),
            status: "failed",
            passed_count: 0,
            failed_count: 1,
            skipped_count: 0,
            exit_code: Some(1),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();

        let overview = build_task_evidence_overview(&db, &project_id, &task_id).unwrap();
        assert_eq!(overview.result_generation, 1);
        let gen0 = overview
            .tests
            .iter()
            .find(|t| t.evidence.result_generation == 0)
            .unwrap();
        assert!(gen0.evidence.status == "passed");
        assert!(!gen0.is_current_generation);
        let gen1 = overview
            .tests
            .iter()
            .find(|t| t.evidence.result_generation == 1)
            .unwrap();
        assert!(gen1.is_current_generation);

        let gen0_timeline_entry = overview
            .timeline
            .iter()
            .find(|e| e.event_type == "test_evidence" && e.result_generation == Some(0))
            .unwrap();
        assert!(gen0_timeline_entry.is_historical);
        let gen1_timeline_entry = overview
            .timeline
            .iter()
            .find(|e| e.event_type == "test_evidence" && e.result_generation == Some(1))
            .unwrap();
        assert!(!gen1_timeline_entry.is_historical);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_generation_zero_acceptance_never_authorizes_completion_of_a_later_generation() {
        let dir = temp_dir("stale-acceptance");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        // Reaching `preview_ready` for the first time bumps
        // `result_generation` 0 -> 1, so the real acceptance row below is
        // genuinely recorded at generation 1 (not 0).
        drive_to_awaiting_acceptance(&db, &task_id);
        db.record_user_acceptance(&project_id, &task_id, true, None)
            .unwrap();
        // Force the task's own generation forward *again* without ever
        // recording a *new* acceptance row for it — deliberately bypassing
        // the real orchestration (which would never normally keep running
        // a task that is already `completed`) purely to prove
        // `is_completed` itself, not the state machine, is what refuses to
        // honor a stale acceptance.
        db.conn()
            .execute(
                "UPDATE tasks SET result_generation = 2, state = 'awaiting_user_acceptance' WHERE id = ?1",
                rusqlite::params![task_id],
            )
            .unwrap();

        let overview = build_task_evidence_overview(&db, &project_id, &task_id).unwrap();
        assert_eq!(overview.result_generation, 2);
        assert!(
            !overview.is_completed,
            "an accepted=true row for generation 1 must never authorize completion of generation 2"
        );
        assert_eq!(overview.acceptance_history.len(), 1);
        assert_eq!(overview.acceptance_history[0].result_generation, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn not_applicable_status_only_appears_alongside_its_legitimizing_confirmation() {
        let dir = temp_dir("not-applicable");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, Some("no framework"))
            .unwrap();
        db.record_test_evidence(&RecordTestEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            test_command_redacted: None,
            status: "no_tests_applicable",
            passed_count: 0,
            failed_count: 0,
            skipped_count: 0,
            exit_code: None,
            reason: Some("no framework"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();

        let overview = build_task_evidence_overview(&db, &project_id, &task_id).unwrap();
        let evidence_entry = overview
            .timeline
            .iter()
            .find(|e| e.event_type == "test_evidence")
            .unwrap();
        assert_eq!(evidence_entry.status, "not_applicable");
        assert!(overview
            .timeline
            .iter()
            .any(|e| e.event_type == "no_tests_confirmed"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_orphaned_no_tests_applicable_row_never_shows_as_not_applicable_in_the_timeline() {
        // The real write path (`Database::record_test_evidence`) now
        // refuses to create this row at all — insert it via raw SQL to
        // prove the *read* layer also fails closed on its own, for any
        // orphaned row that could exist regardless of how it got there
        // (e.g. data written before this remediation).
        let dir = temp_dir("orphaned-not-applicable");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        db.conn()
            .execute(
                "INSERT INTO test_evidence (id, project_id, task_id, result_generation, round, test_command_redacted, status, passed_count, failed_count, skipped_count, exit_code, reason, stdout_ref, stderr_ref, created_at)
                 VALUES (?1, ?2, ?3, 0, 1, NULL, 'no_tests_applicable', 0, 0, 0, NULL, 'orphaned', NULL, NULL, ?4)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    project_id,
                    task_id,
                    chrono::Utc::now().to_rfc3339(),
                ],
            )
            .unwrap();

        let overview = build_task_evidence_overview(&db, &project_id, &task_id).unwrap();
        let evidence_entry = overview
            .timeline
            .iter()
            .find(|e| e.event_type == "test_evidence")
            .unwrap();
        assert_eq!(
            evidence_entry.status, "insufficient_evidence",
            "an orphaned no_tests_applicable row with no matching confirmation must fail closed, \
             never display as not_applicable"
        );
        assert!(
            !overview.acceptance_ready,
            "an orphaned not-applicable row must never make the task look acceptance-ready"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_cancelled_timed_out_and_insufficient_evidence_remain_distinct() {
        let dir = temp_dir("distinct-statuses");
        let (db, project_id, task_id) = db_with_task(&dir, "P");
        for (generation, round, status) in [
            (0_i64, 1_i64, "failed"),
            (0_i64, 2_i64, "cancelled"),
            (1_i64, 1_i64, "timed_out"),
            (1_i64, 2_i64, "insufficient_evidence"),
        ] {
            db.record_build_evidence(&RecordBuildEvidenceRequest {
                project_id: &project_id,
                task_id: &task_id,
                result_generation: generation,
                round,
                build_command_redacted: Some("cargo build"),
                status,
                authorization_source: None,
                exit_code: None,
                reason: None,
                stdout_ref: None,
                stderr_ref: None,
            })
            .unwrap();
        }
        // Force `result_generation` to 1 so both generations are in range
        // of the overview's own `0..=task.result_generation` scan.
        db.conn()
            .execute(
                "UPDATE tasks SET result_generation = 1 WHERE id = ?1",
                rusqlite::params![task_id],
            )
            .unwrap();

        let overview = build_task_evidence_overview(&db, &project_id, &task_id).unwrap();
        let statuses: Vec<&str> = overview
            .builds
            .iter()
            .map(|b| b.evidence.status.as_str())
            .collect();
        for expected in ["failed", "cancelled", "timed_out", "insufficient_evidence"] {
            assert!(
                statuses.contains(&expected),
                "expected status {expected} to survive untouched, got {statuses:?}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overview_facts_survive_a_real_file_backed_database_reopen() {
        let dir = temp_dir("reopen");
        let db_path = dir.join("nova.sqlite3");
        let (project_id, task_id) = {
            let db = Database::open(&db_path).unwrap();
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
            db.record_test_evidence(&RecordTestEvidenceRequest {
                project_id: &project.id,
                task_id: &task.id,
                result_generation: 0,
                round: 1,
                test_command_redacted: Some("cargo test"),
                status: "passed",
                passed_count: 1,
                failed_count: 0,
                skipped_count: 0,
                exit_code: Some(0),
                reason: None,
                stdout_ref: None,
                stderr_ref: None,
            })
            .unwrap();
            (project.id, task.id)
        };

        let before = {
            let db = Database::open(&db_path).unwrap();
            build_task_evidence_overview(&db, &project_id, &task_id).unwrap()
        };
        let after = {
            let db = Database::open(&db_path).unwrap();
            build_task_evidence_overview(&db, &project_id, &task_id).unwrap()
        };
        assert_eq!(
            before, after,
            "the exact same database facts must derive the exact same overview across a reopen"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
