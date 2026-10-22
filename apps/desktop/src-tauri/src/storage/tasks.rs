use rusqlite::params;

use crate::db::Database;
use crate::error::{NovaError, NovaResult};
use crate::models::{Actor, ActionStatus, CreateTaskRequest, Task, TaskState, TaskStateEvent};
use crate::security::redact_sensitive_text;

use super::{new_id, now};

#[cfg(test)]
thread_local! {
    // One-shot barrier for deterministic two-connection transition races.
    pub(crate) static BEFORE_TRANSITION_LOCK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

/// WP2A: why [`Database::transition_task`] refused a `to == Completed`
/// attempt. Never itself written anywhere — it is a pure in-memory
/// classification of already-persisted data, recomputed fresh on every
/// call (including by a read-only precheck a caller can run before even
/// attempting the transition, so the UI can explain *why* a "mark
/// complete" action is currently disabled without actually attempting it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionRejectReason {
    /// The task's historical AI-mode could not be confirmed as a clean,
    /// positively-evidenced single-AI run (`legacy_classification` is
    /// `dual_ai_evidenced` or `unknown_insufficient`) — frozen at migration
    /// time, never re-evaluated for a task created afterward.
    LegacyTaskFrozen,
    InvalidCompletionState,
    /// No `(generation, round)` has both a successfully-completed dev/fix
    /// action and real passing test+build evidence (or valid no-tests/
    /// no-build authorization) attributed to that exact action's current
    /// attempt. Should not be reachable in practice once a task has
    /// legitimately reached `awaiting_user_acceptance` — kept as a
    /// defensive, never-expected-to-fire check.
    NoAdoptedEvidence,
    /// The dev/fix action for the otherwise-adopted round is missing or
    /// not `Succeeded`. Same defensive status as `NoAdoptedEvidence`.
    DevActionNotSucceeded,
    TestAttemptMismatch,
    BuildAttemptMismatch,
    SelfCheckMissing,
    SelfCheckInvalidFormat,
    SelfCheckNotComplete,
    UnfinishedItemsPresent,
    BlockingIssuesPresent,
    RequirementSnapshotMissing,
    RequirementCoverageIncomplete,
    UserAcceptanceMissing,
}

impl CompletionRejectReason {
    /// A stable, machine-readable identifier for this reason — for a
    /// frontend that wants to branch on *which* reason it is (e.g. to show
    /// a different follow-up action) without parsing the Chinese
    /// [`message`](Self::message) text.
    pub fn code(&self) -> &'static str {
        match self {
            Self::LegacyTaskFrozen => "legacy_task_frozen",
            Self::InvalidCompletionState => "invalid_completion_state",
            Self::NoAdoptedEvidence => "no_adopted_evidence",
            Self::DevActionNotSucceeded => "dev_action_not_succeeded",
            Self::TestAttemptMismatch => "test_attempt_mismatch",
            Self::BuildAttemptMismatch => "build_attempt_mismatch",
            Self::SelfCheckMissing => "self_check_missing",
            Self::SelfCheckInvalidFormat => "self_check_invalid_format",
            Self::SelfCheckNotComplete => "self_check_not_complete",
            Self::UnfinishedItemsPresent => "unfinished_items_present",
            Self::BlockingIssuesPresent => "blocking_issues_present",
            Self::RequirementSnapshotMissing => "requirement_snapshot_missing",
            Self::RequirementCoverageIncomplete => "requirement_coverage_incomplete",
            Self::UserAcceptanceMissing => "user_acceptance_missing",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::LegacyTaskFrozen => "该任务的历史执行路径无法确认，已冻结，不能完成",
            Self::InvalidCompletionState => "只有等待用户验收的结果可以提交验收或完成",
            Self::NoAdoptedEvidence => "尚未找到真实通过的测试与构建证据",
            Self::DevActionNotSucceeded => "开发/修复执行记录异常，无法确认归属",
            Self::TestAttemptMismatch => "采纳结果的测试证据与当前开发执行不匹配",
            Self::BuildAttemptMismatch => "采纳结果的构建证据与当前开发执行不匹配",
            Self::SelfCheckMissing => "缺少当前轮次的自检记录",
            Self::SelfCheckInvalidFormat => "自检记录格式无效，无法解析",
            Self::SelfCheckNotComplete => "开发AI自检认为尚未完成或被阻断",
            Self::UnfinishedItemsPresent => "自检记录中存在未完成项",
            Self::BlockingIssuesPresent => "自检记录中存在阻断问题",
            Self::RequirementSnapshotMissing => "任务缺少已确认需求快照，无法核对覆盖情况",
            Self::RequirementCoverageIncomplete => "自检的需求覆盖记录不完整或不一致",
            Self::UserAcceptanceMissing => "缺少当前结果的用户确认",
        }
    }
}

/// `tasks.legacy_classification` values that block the entry points listed
/// in the WP2A design docs (`run_core_loop`, `unblock_test_execution_for_retry`,
/// `confirm_no_tests_applicable`/`confirm_no_build_applicable`, and this
/// module's own `to IN (Queued, Running, Completed)` checks). `None` and
/// `"single_ai_evidenced"` (never actually assigned under the current
/// schema, kept for forward compatibility) are never frozen.
pub fn is_legacy_frozen(task: &Task) -> bool {
    matches!(
        task.legacy_classification.as_deref(),
        Some("dual_ai_evidenced") | Some("unknown_insufficient")
    )
}

#[derive(serde::Deserialize)]
struct CoverageEntry {
    item: String,
    status: String,
}

/// Every legal `(from, to)` transition. Anything not listed here is
/// rejected by [`is_legal_transition`] — deny-by-default, not
/// deny-by-exception.
///
/// Design notes (see the Phase 2 report for the full rationale):
/// - `completed` is reachable only from `awaiting_user_acceptance`, and
///   [`Database::transition_task`] additionally requires a `user_acceptance`
///   row before allowing that specific edge — satisfies "completed 只能从
///   awaiting_user_acceptance 进入" and "completed 必须有用户确认记录".
/// - `failed` and `cancelled` have no edge into `completed` at all, from
///   either state, automatic or not — satisfies "failed/cancelled 不能自动
///   进入 completed".
/// - `cancelled` and `completed` have zero outgoing edges (fully
///   terminal) — satisfies "cancelled 不能自动恢复为 running" by construction
///   rather than by runtime check.
/// - There is no `first_review`/`second_review` state and no transition
///   name implies one — this is the single-AI v0.1.0 loop; a second
///   reviewing AI is out of scope for Phase 2 (and for v0.1.0 entirely).
/// - The `safe_fixing`/`verifying` loop is structurally legal (needed for
///   ordinary "found an issue, fixed it, re-verify" cycles) but is capped
///   at runtime by [`Database::transition_task`] via
///   [`assert_fix_cycle_allowed`] — satisfies "不允许无限自动修复循环".
const LEGAL_TRANSITIONS: &[(TaskState, TaskState)] = &[
    (TaskState::Draft, TaskState::AwaitingConfirmation),
    (TaskState::Draft, TaskState::Cancelled),
    (TaskState::AwaitingConfirmation, TaskState::Confirmed),
    (TaskState::AwaitingConfirmation, TaskState::Draft),
    (TaskState::AwaitingConfirmation, TaskState::Cancelled),
    (TaskState::Confirmed, TaskState::Queued),
    (TaskState::Confirmed, TaskState::Cancelled),
    (TaskState::Queued, TaskState::Running),
    (TaskState::Queued, TaskState::Cancelled),
    (TaskState::Queued, TaskState::Interrupted),
    (TaskState::Running, TaskState::Verifying),
    (TaskState::Running, TaskState::NeedsUserAction),
    (TaskState::Running, TaskState::Interrupted),
    (TaskState::Running, TaskState::Failed),
    (TaskState::Running, TaskState::Cancelled),
    (TaskState::Verifying, TaskState::SafeFixing),
    (TaskState::Verifying, TaskState::PreviewReady),
    (TaskState::Verifying, TaskState::NeedsUserAction),
    (TaskState::Verifying, TaskState::Interrupted),
    (TaskState::Verifying, TaskState::Failed),
    (TaskState::Verifying, TaskState::Cancelled),
    (TaskState::SafeFixing, TaskState::Verifying),
    (TaskState::SafeFixing, TaskState::NeedsUserAction),
    (TaskState::SafeFixing, TaskState::Interrupted),
    (TaskState::SafeFixing, TaskState::Failed),
    (TaskState::SafeFixing, TaskState::Cancelled),
    (TaskState::PreviewReady, TaskState::AwaitingUserAcceptance),
    (TaskState::PreviewReady, TaskState::SafeFixing),
    (TaskState::PreviewReady, TaskState::Interrupted),
    (TaskState::PreviewReady, TaskState::Cancelled),
    (TaskState::AwaitingUserAcceptance, TaskState::Completed),
    (
        TaskState::AwaitingUserAcceptance,
        TaskState::NeedsUserAction,
    ),
    (TaskState::AwaitingUserAcceptance, TaskState::SafeFixing),
    (TaskState::AwaitingUserAcceptance, TaskState::Interrupted),
    (TaskState::AwaitingUserAcceptance, TaskState::Cancelled),
    (TaskState::NeedsUserAction, TaskState::Running),
    (TaskState::NeedsUserAction, TaskState::Verifying),
    (TaskState::NeedsUserAction, TaskState::SafeFixing),
    (TaskState::NeedsUserAction, TaskState::Queued),
    (TaskState::NeedsUserAction, TaskState::Cancelled),
    (TaskState::NeedsUserAction, TaskState::Interrupted),
    (TaskState::Interrupted, TaskState::Queued),
    (TaskState::Interrupted, TaskState::NeedsUserAction),
    (TaskState::Interrupted, TaskState::Cancelled),
    (TaskState::Failed, TaskState::AwaitingConfirmation),
    (TaskState::Failed, TaskState::Cancelled),
];

/// A task may automatically re-enter `safe_fixing` at most this many times
/// per task. The limit only applies to non-user-initiated transitions
/// (`actor != User`) — a human explicitly asking for one more fix pass is
/// not an "infinite automatic loop".
pub const MAX_AUTO_FIX_CYCLES: u32 = 3;

pub fn is_legal_transition(from: TaskState, to: TaskState) -> bool {
    LEGAL_TRANSITIONS.contains(&(from, to))
}

/// Pure guard for the "no infinite automatic fix loop" rule: `existing_count`
/// is how many times this task has already entered `safe_fixing`
/// automatically; returns `Err` once the cap is reached so the caller must
/// route to `needs_user_action` (a human decision) instead.
pub fn assert_fix_cycle_allowed(existing_count: u32) -> Result<(), String> {
    if existing_count >= MAX_AUTO_FIX_CYCLES {
        Err(format!(
            "自动修复次数已达到上限（{MAX_AUTO_FIX_CYCLES} 次），需要用户介入才能继续"
        ))
    } else {
        Ok(())
    }
}

impl Database {
    pub fn create_task(&self, request: &CreateTaskRequest, actor: Actor) -> NovaResult<Task> {
        self.conn().execute("BEGIN IMMEDIATE;", [])?;
        let result = self.create_task_in_transaction(request, actor);
        match result {
            Ok(task) => {
                self.conn().execute("COMMIT;", [])?;
                Ok(task)
            }
            Err(error) => {
                let _ = self.conn().execute("ROLLBACK;", []);
                Err(error)
            }
        }
    }

    /// Caller owns the write transaction. Shared with requirement confirmation.
    pub(crate) fn create_task_in_transaction(
        &self,
        request: &CreateTaskRequest,
        actor: Actor,
    ) -> NovaResult<Task> {
        assert!(!self.conn().is_autocommit(), "requires write transaction");
        let id = new_id();
        let now = now();
        self.conn().execute(
                "INSERT INTO tasks (id, project_id, title, goal, in_scope, not_in_scope, done_criteria, ai_kind, state, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    id,
                    request.project_id,
                    request.title,
                    request.goal,
                    request.in_scope,
                    request.not_in_scope,
                    request.done_criteria,
                    request.ai_kind,
                    TaskState::Draft.as_str(),
                    now,
                ],
            )?;
        self.conn().execute(
                "INSERT INTO task_state_events (id, project_id, task_id, old_state, new_state, reason, actor, correlation_id, is_recovery, safety_summary, created_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, NULL, 0, NULL, ?7)",
                params![new_id(), request.project_id, id, TaskState::Draft.as_str(), "任务已创建", actor.as_str(), now],
            )?;
        self.task(&id)
    }

    pub fn task(&self, id: &str) -> NovaResult<Task> {
        self.conn()
            .query_row(
                "SELECT id, project_id, title, goal, in_scope, not_in_scope, done_criteria, ai_kind, state, result_generation, test_command_redacted, test_command_source, document_sync_pending, legacy_classification, created_at, updated_at
                 FROM tasks WHERE id = ?1",
                params![id],
                read_task,
            )
            .map_err(Into::into)
    }

    /// Phase 3 remediation (Blocker 3): records the test command Nova will
    /// run independently for this task — either auto-detected
    /// (`source = "auto"`) or explicitly confirmed by the user
    /// (`source = "user"`). `source` is validated against exactly those two
    /// values; never anything derived from AI output. Redacted before
    /// storage like every other free-text field a task carries.
    pub fn set_task_test_command(
        &self,
        task_id: &str,
        command_redacted: &str,
        source: &str,
    ) -> NovaResult<Task> {
        if source != "auto" && source != "user" {
            return Err(NovaError::Config(format!(
                "test_command_source 必须是 auto 或 user，实际为：{source}"
            )));
        }
        let command_redacted = redact_sensitive_text(command_redacted);
        self.conn().execute(
            "UPDATE tasks SET test_command_redacted = ?1, test_command_source = ?2, updated_at = ?3 WHERE id = ?4",
            params![command_redacted, source, now(), task_id],
        )?;
        self.task(task_id)
    }

    /// Second re-review remediation (Major 5): flips `document_sync_pending`
    /// — set to `true` when a follow-up seven-document regeneration failed
    /// after an otherwise-successful acceptance/rejection/completion write
    /// (see `commands::accept_task_result`), cleared back to `false` once a
    /// later regeneration attempt succeeds. Never itself fails the caller's
    /// larger operation — this is a best-effort marker, not a gate.
    pub fn set_document_sync_pending(&self, task_id: &str, pending: bool) -> NovaResult<()> {
        self.conn().execute(
            "UPDATE tasks SET document_sync_pending = ?1, updated_at = ?2 WHERE id = ?3",
            params![pending as i64, now(), task_id],
        )?;
        Ok(())
    }

    pub fn list_tasks_for_project(&self, project_id: &str) -> NovaResult<Vec<Task>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, title, goal, in_scope, not_in_scope, done_criteria, ai_kind, state, result_generation, test_command_redacted, test_command_source, document_sync_pending, legacy_classification, created_at, updated_at
             FROM tasks WHERE project_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map(params![project_id], read_task)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The most recent task for a project that is not in a terminal state
    /// (`completed`/`cancelled`/`failed`) — the "current unfinished task"
    /// the recovery service and the minimal resume UI care about.
    pub fn unfinished_task_for_project(&self, project_id: &str) -> NovaResult<Option<Task>> {
        self.conn()
            .query_row(
                "SELECT id, project_id, title, goal, in_scope, not_in_scope, done_criteria, ai_kind, state, result_generation, test_command_redacted, test_command_source, document_sync_pending, legacy_classification, created_at, updated_at
                 FROM tasks
                 WHERE project_id = ?1 AND state NOT IN ('completed', 'cancelled', 'failed')
                 ORDER BY updated_at DESC LIMIT 1",
                params![project_id],
                read_task,
            )
            .optional_result()
    }

    /// Phase 4 remediation (Major 1): requires `project_id` and verifies
    /// `task.project_id == project_id` — via the same
    /// `assert_task_belongs_to_project` guard every evidence-reading
    /// function in this codebase already uses — before ever touching
    /// `task_state_events`. Previously this accepted only `task_id`, so a
    /// caller that merely knew or guessed a `task_id` from a different
    /// project could read its full state history (reasons, actor, recovery
    /// flag, safety summary, timestamps). Pushing the check down here
    /// (rather than leaving it solely at whichever call site happens to
    /// check first) makes it structurally impossible for any future direct
    /// caller to skip it.
    pub fn list_task_state_events(
        &self,
        project_id: &str,
        task_id: &str,
    ) -> NovaResult<Vec<TaskStateEvent>> {
        self.assert_task_belongs_to_project(project_id, task_id)?;
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, old_state, new_state, reason, actor, correlation_id, is_recovery, safety_summary, created_at
             FROM task_state_events WHERE task_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id], read_event)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn count_automatic_safe_fixing_entries(&self, task_id: &str) -> NovaResult<u32> {
        let count: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM task_state_events WHERE task_id = ?1 AND new_state = 'safe_fixing' AND actor != 'user' AND is_recovery = 0",
            params![task_id],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u32)
    }

    /// WP2A: the full `completed` gate — read-only, callable standalone as
    /// a precheck (so the UI can explain why "mark complete" is disabled
    /// without attempting it) and also called by [`Database::transition_task`]
    /// itself immediately before its own atomic `UPDATE`, inside the same
    /// `BEGIN IMMEDIATE` transaction, so there is no window between this
    /// check and the write where a concurrent writer could invalidate it.
    ///
    /// Finds the `(round, dev_attempt)` whose dev/fix action succeeded and
    /// whose test+build evidence (or no-tests/no-build authorization) is
    /// attributed to that *exact* attempt — never just "some evidence for
    /// this generation/round", which is what [`NoAdoptedEvidence`] /
    /// [`DevActionNotSucceeded`] exist to catch defensively (should not be
    /// reachable once a task has legitimately reached
    /// `awaiting_user_acceptance`, since `result_attempt` binding already
    /// prevents a stale attempt's evidence from ever being written in the
    /// first place — see `core_loop::run_tests_for_round`/
    /// `run_build_for_round`). Then requires: a current, valid, complete
    /// self-check with no unfinished/blocking items; full requirement
    /// coverage against [`Database::list_confirmed_requirement_items`];
    /// and a real `accepted = 1` `user_acceptance` row for the task's
    /// current `result_generation`.
    ///
    /// [`NoAdoptedEvidence`]: CompletionRejectReason::NoAdoptedEvidence
    /// [`DevActionNotSucceeded`]: CompletionRejectReason::DevActionNotSucceeded
    pub fn evaluate_completion_gate(
        &self,
        task: &Task,
    ) -> NovaResult<Result<(), CompletionRejectReason>> {
        self.evaluate_result_gate(task, true)
    }

    /// Predict the explicit accept operation, which records the user's decision
    /// before completion. This does not grant direct completion permission.
    pub fn evaluate_acceptance_submission(&self, task: &Task) -> NovaResult<Result<(), CompletionRejectReason>> {
        self.evaluate_result_gate(task, false)
    }

    fn evaluate_result_gate(&self, task: &Task, require_acceptance: bool) -> NovaResult<Result<(), CompletionRejectReason>> {
        if is_legacy_frozen(task) {
            return Ok(Err(CompletionRejectReason::LegacyTaskFrozen));
        }
        if task.state != TaskState::AwaitingUserAcceptance {
            return Ok(Err(CompletionRejectReason::InvalidCompletionState));
        }

        // Freeze the authoritative adoption before checking attribution. A failed
        // check must never search another generation/round for a rescue.
        let Some((generation, adopted_round)) = crate::core_loop::documents::adopted_passing_generation_round(
            self, &task.project_id, &task.id, task,
        )? else {
            return Ok(Err(CompletionRejectReason::NoAdoptedEvidence));
        };
        let dev_key = format!("dev-{}-round-{adopted_round}", task.id);
        let Some(dev_action) = self.find_action(&task.id, &dev_key)? else {
            return Ok(Err(CompletionRejectReason::DevActionNotSucceeded));
        };
        if dev_action.status != ActionStatus::Succeeded {
            return Ok(Err(CompletionRejectReason::DevActionNotSucceeded));
        }
        let current_attempt = dev_action.attempt_number;
        let mut test_ok = false;
        for evidence in self.list_test_evidence_for_round(&task.project_id, &task.id, generation, adopted_round)? {
            if evidence.result_attempt != Some(current_attempt) { continue; }
            if evidence.status == "passed" || (evidence.status == "no_tests_applicable"
                && crate::core_loop::documents::no_tests_applicable_is_authorized(
                    self, &task.project_id, &task.id, generation, adopted_round, Some(current_attempt),
                )?) { test_ok = true; break; }
        }
        if !test_ok { return Ok(Err(CompletionRejectReason::TestAttemptMismatch)); }
        let mut build_ok = false;
        for evidence in self.list_build_evidence_for_round(&task.project_id, &task.id, generation, adopted_round)? {
            if evidence.result_attempt != Some(current_attempt) { continue; }
            if evidence.status == "passed" || (evidence.status == "no_build_applicable"
                && crate::core_loop::documents::no_build_applicable_is_authorized(
                    self, &task.project_id, &task.id, generation, adopted_round, Some(current_attempt),
                    evidence.authorization_source.as_deref(),
                )?) { build_ok = true; break; }
        }
        if !build_ok { return Ok(Err(CompletionRejectReason::BuildAttemptMismatch)); }

        let Some(self_check) = self.current_self_check(&task.id, adopted_round, current_attempt)?
        else {
            return Ok(Err(CompletionRejectReason::SelfCheckMissing));
        };
        if self_check.parse_status != "valid" {
            return Ok(Err(CompletionRejectReason::SelfCheckInvalidFormat));
        }
        if self_check.self_reported_status.as_deref() != Some("self_reported_complete") {
            return Ok(Err(CompletionRejectReason::SelfCheckNotComplete));
        }
        let Some(unfinished): Option<Vec<String>> = self_check.unfinished_items_json.as_deref()
            .and_then(|json| serde_json::from_str(json).ok()) else {
            return Ok(Err(CompletionRejectReason::SelfCheckInvalidFormat));
        };
        if !unfinished.is_empty() {
            return Ok(Err(CompletionRejectReason::UnfinishedItemsPresent));
        }
        let Some(blocking): Option<Vec<String>> = self_check.blocking_issues_json.as_deref()
            .and_then(|json| serde_json::from_str(json).ok()) else {
            return Ok(Err(CompletionRejectReason::SelfCheckInvalidFormat));
        };
        if !blocking.is_empty() {
            return Ok(Err(CompletionRejectReason::BlockingIssuesPresent));
        }

        let authoritative = self.list_confirmed_requirement_items(&task.id)?;
        if authoritative.is_empty() {
            return Ok(Err(CompletionRejectReason::RequirementSnapshotMissing));
        }
        let coverage: Vec<CoverageEntry> = self_check
            .requirement_coverage_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        let authoritative_keys: std::collections::BTreeSet<String> = authoritative
            .iter()
            .map(|item| format!("{}:{}", item.field, item.item_index))
            .collect();
        let mut seen_keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut coverage_ok = !coverage.is_empty();
        for entry in &coverage {
            if entry.status != "covered" {
                coverage_ok = false;
            }
            if !authoritative_keys.contains(&entry.item) {
                coverage_ok = false; // fabricated/extra item
            }
            if !seen_keys.insert(entry.item.clone()) {
                coverage_ok = false; // duplicate item
            }
        }
        if seen_keys.len() != authoritative_keys.len() {
            coverage_ok = false; // missing item(s)
        }
        if !coverage_ok {
            return Ok(Err(CompletionRejectReason::RequirementCoverageIncomplete));
        }

        let accepted: i64 = self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM user_acceptance WHERE task_id = ?1 AND result_generation = ?2 AND accepted = 1)",
            params![task.id, task.result_generation],
            |row| row.get(0),
        )?;
        if require_acceptance && accepted == 0 {
            return Ok(Err(CompletionRejectReason::UserAcceptanceMissing));
        }

        Ok(Ok(()))
    }

    /// The single authoritative entry point for moving a task from one
    /// state to another. Validates the transition against
    /// [`LEGAL_TRANSITIONS`] and the automatic-fix-cycle cap up front, then
    /// performs the state write, the `completed` acceptance-generation
    /// gate, and the `task_state_events` audit insert as one atomic
    /// conditional write — see the inline comment on the `UPDATE` below for
    /// why the completion gate itself is embedded in the `WHERE` clause
    /// rather than checked beforehand.
    #[allow(clippy::too_many_arguments)]
    pub fn transition_task(
        &self,
        task_id: &str,
        to: TaskState,
        actor: Actor,
        reason: Option<&str>,
        correlation_id: Option<&str>,
        is_recovery: bool,
        safety_summary: Option<&str>,
    ) -> NovaResult<Task> {
        #[cfg(test)]
        BEFORE_TRANSITION_LOCK.with(|hook| {
            if let Some(callback) = hook.borrow_mut().take() {
                callback();
            }
        });
        self.conn().execute("BEGIN IMMEDIATE;", [])?;
        let result = self.transition_task_in_transaction(
            task_id,
            to,
            actor,
            reason,
            correlation_id,
            is_recovery,
            safety_summary,
        );
        match result {
            Ok(task) => {
                self.conn().execute("COMMIT;", [])?;
                Ok(task)
            }
            Err(error) => {
                let _ = self.conn().execute("ROLLBACK;", []);
                Err(error)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn transition_task_in_transaction(
        &self,
        task_id: &str,
        to: TaskState,
        actor: Actor,
        reason: Option<&str>,
        correlation_id: Option<&str>,
        is_recovery: bool,
        safety_summary: Option<&str>,
    ) -> NovaResult<Task> {
        assert!(!self.conn().is_autocommit(), "requires write transaction");
        let reason = reason.map(redact_sensitive_text);
        let safety_summary = safety_summary.map(redact_sensitive_text);
        let now = now();
        let task = self.task(task_id)?;
        let from = task.state;

        if !is_legal_transition(from, to) {
            return Err(NovaError::Config(format!(
                "非法的任务状态转换：{from} -> {to}"
            )));
        }

        // Read and validate under the write lock. No state, event, action,
        // or confirmation is changed by a rejected frozen transition.
        if (to == TaskState::Queued || to == TaskState::Running) && is_legacy_frozen(&task) {
            return Err(NovaError::Config(
                CompletionRejectReason::LegacyTaskFrozen
                    .message()
                    .to_string(),
            ));
        }

        if to == TaskState::SafeFixing && actor != Actor::User && !is_recovery {
            let existing = self.count_automatic_safe_fixing_entries(task_id)?;
            assert_fix_cycle_allowed(existing).map_err(NovaError::Config)?;
        }

        // WP2A (B2): the full completed gate is evaluated here, inside
        // this same transaction, immediately before the `UPDATE`
        // below — not as a separate, earlier, independently-committed
        // step. Its boolean result is folded into the same `WHERE`
        // clause the pre-existing user-acceptance check already used,
        // preserving the exact TOCTOU-closing property that check's
        // own comment describes: there is no window between "we
        // decided completion is allowed" and "we wrote it" where a
        // concurrent connection could invalidate the decision.
        let completion_reason: Option<CompletionRejectReason> = if to == TaskState::Completed {
            self.evaluate_completion_gate(&task)?.err()
        } else {
            None
        };
        let completion_ok = completion_reason.is_none();

        let rows_affected = self.conn().execute(
            "UPDATE tasks
                 SET state = ?1, updated_at = ?2
                 WHERE id = ?3
                   AND state = ?4
                   AND (?1 <> 'completed' OR ?5 = 1)",
            params![
                to.as_str(),
                now,
                task_id,
                from.as_str(),
                completion_ok as i64
            ],
        )?;
        if rows_affected == 0 {
            // Diagnose why, purely for a clear error message — this
            // read is not authoritative for the decision itself, which
            // was already made atomically above.
            let current = self.task(task_id)?;
            if current.state != from {
                return Err(NovaError::Config(format!(
                    "任务状态已被并发修改，{from} -> {to} 已取消（当前状态不再是 {from}）"
                )));
            }
            if to == TaskState::Completed {
                let message = completion_reason
                    .map(|r| r.message())
                    .unwrap_or("无法进入 completed：完工门禁未通过");
                return Err(NovaError::Config(format!("无法进入 completed：{message}")));
            }
            return Err(NovaError::Config(format!(
                "任务状态转换失败：{from} -> {to}"
            )));
        }

        // A fresh result supersedes any acceptance recorded for an
        // earlier generation — bumping here, inside the same
        // transaction as the state write, is what makes "old
        // acceptance can't authorize a new result" hold even under
        // concurrency: no other writer can observe the task as
        // `preview_ready` with the old `result_generation` still in
        // effect.
        if to == TaskState::PreviewReady {
            self.conn().execute(
                "UPDATE tasks SET result_generation = result_generation + 1 WHERE id = ?1",
                params![task_id],
            )?;
        }

        self.conn().execute(
                "INSERT INTO task_state_events (id, project_id, task_id, old_state, new_state, reason, actor, correlation_id, is_recovery, safety_summary, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    new_id(),
                    task.project_id,
                    task_id,
                    from.as_str(),
                    to.as_str(),
                    reason,
                    actor.as_str(),
                    correlation_id,
                    is_recovery as i64,
                    safety_summary,
                    now,
                ],
            )?;

        // WP2A-0: the moment a task leaves `Draft` for `AwaitingConfirmation`
        // *is* "the user confirmed the requirement" — this is the one
        // and only place a fresh task's requirement snapshot is taken,
        // in the same transaction as the state write itself. Silently
        // a no-op if a snapshot already exists for this task (it never
        // will yet, for a task taking this edge for the first and only
        // legal time, but the check is defensive, not load-bearing).
        if to == TaskState::AwaitingConfirmation {
            self.insert_confirmed_requirement_items_if_absent(&task.project_id, &task)?;
        }

        self.task(task_id)
    }

}

/// Small helper trait so `query_row(...).optional()` composes with our
/// `NovaResult` instead of `rusqlite::Result` at call sites that need
/// "not found is fine, return `None`".
trait OptionalResultExt<T> {
    fn optional_result(self) -> NovaResult<Option<T>>;
}

impl<T> OptionalResultExt<T> for rusqlite::Result<T> {
    fn optional_result(self) -> NovaResult<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

pub(super) fn parse_enum<T: std::str::FromStr<Err = String>>(value: String) -> rusqlite::Result<T> {
    value.parse().map_err(|error: String| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, error.into())
    })
}

fn read_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let state: String = row.get(8)?;
    Ok(Task {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        goal: row.get(3)?,
        in_scope: row.get(4)?,
        not_in_scope: row.get(5)?,
        done_criteria: row.get(6)?,
        ai_kind: row.get(7)?,
        state: parse_enum(state)?,
        result_generation: row.get(9)?,
        test_command_redacted: row.get(10)?,
        test_command_source: row.get(11)?,
        document_sync_pending: row.get::<_, i64>(12)? != 0,
        legacy_classification: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn read_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskStateEvent> {
    let old_state: Option<String> = row.get(3)?;
    let new_state: String = row.get(4)?;
    let actor: String = row.get(6)?;
    let is_recovery: i64 = row.get(8)?;
    Ok(TaskStateEvent {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        old_state: old_state.map(parse_enum).transpose()?,
        new_state: parse_enum(new_state)?,
        reason: row.get(5)?,
        actor: parse_enum(actor)?,
        correlation_id: row.get(7)?,
        is_recovery: is_recovery != 0,
        safety_summary: row.get(9)?,
        created_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CreateProjectRequest;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-tasks-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db_with_project(dir: &std::path::Path) -> (Database, String) {
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
        (db, project.id)
    }

    fn new_task(db: &Database, project_id: &str) -> Task {
        db.create_task(
            &CreateTaskRequest {
                project_id: project_id.to_string(),
                title: "示例任务".to_string(),
                goal: Some("目标".to_string()),
                in_scope: None,
                not_in_scope: None,
                done_criteria: None,
                ai_kind: None,
            },
            Actor::User,
        )
        .unwrap()
    }

    #[test]
    fn new_task_starts_in_draft_with_one_state_event() {
        let dir = temp_dir("draft");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        assert_eq!(task.state, TaskState::Draft);
        let events = db.list_task_state_events(&project_id, &task.id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].old_state, None);
        assert_eq!(events[0].new_state, TaskState::Draft);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Phase 4 remediation (Major 1): list_task_state_events ownership ---

    #[test]
    fn list_task_state_events_returns_real_events_for_the_correct_project_and_task() {
        let dir = temp_dir("state-events-correct-owner");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        let events = db.list_task_state_events(&project_id, &task.id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].task_id, task.id);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_task_state_events_rejects_a_project_id_that_does_not_own_the_task() {
        let dir_a = temp_dir("state-events-wrong-project-a");
        let dir_b = temp_dir("state-events-wrong-project-b");
        let (db, _project_a) = db_with_project(&dir_a);
        let task_a = new_task(&db, &_project_a);
        let (_db_unused, project_b) = db_with_project(&dir_b);

        let result = db.list_task_state_events(&project_b, &task_a.id);
        assert!(
            result.is_err(),
            "a project_id that does not own task_a must never be able to read its state events"
        );
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn list_task_state_events_never_leaks_a_sibling_tasks_events_within_the_same_project() {
        let dir = temp_dir("state-events-sibling-tasks");
        let (db, project_id) = db_with_project(&dir);
        let task_a = new_task(&db, &project_id);
        let task_b = new_task(&db, &project_id);
        db.transition_task(
            &task_b.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            Some("only task B's own event"),
            None,
            false,
            None,
        )
        .unwrap();

        let events_a = db.list_task_state_events(&project_id, &task_a.id).unwrap();
        assert!(
            events_a.iter().all(|e| e.task_id == task_a.id),
            "task A's query must never include task B's rows, even within the same project"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_task_state_events_fails_safely_for_a_task_id_that_does_not_exist() {
        let dir = temp_dir("state-events-missing-task");
        let (db, project_id) = db_with_project(&dir);
        let result = db.list_task_state_events(&project_id, "no-such-task-id");
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_documented_legal_transition_is_accepted() {
        for &(from, to) in LEGAL_TRANSITIONS {
            assert!(
                is_legal_transition(from, to),
                "{from} -> {to} should be legal by construction"
            );
        }
    }

    #[test]
    fn a_representative_sample_of_illegal_transitions_is_rejected() {
        let illegal = [
            (TaskState::Draft, TaskState::Completed),
            (TaskState::Draft, TaskState::Running),
            (TaskState::Running, TaskState::Completed),
            (TaskState::Queued, TaskState::Completed),
            (TaskState::Verifying, TaskState::Completed),
            (TaskState::Failed, TaskState::Completed),
            (TaskState::Cancelled, TaskState::Running),
            (TaskState::Cancelled, TaskState::Queued),
            (TaskState::Completed, TaskState::Running),
            (TaskState::Completed, TaskState::AwaitingUserAcceptance),
            (TaskState::SafeFixing, TaskState::Completed),
            (TaskState::NeedsUserAction, TaskState::Completed),
            (TaskState::Interrupted, TaskState::Running),
            (TaskState::Interrupted, TaskState::Completed),
        ];
        for (from, to) in illegal {
            assert!(
                !is_legal_transition(from, to),
                "{from} -> {to} must be rejected"
            );
        }
    }

    #[test]
    fn running_cannot_go_directly_to_completed() {
        let dir = temp_dir("running-direct-complete");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        db.transition_task(
            &task.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Confirmed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let result = db.transition_task(
            &task.id,
            TaskState::Completed,
            Actor::System,
            None,
            None,
            false,
            None,
        );
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    fn drive_to_awaiting_acceptance(db: &Database, task_id: &str) {
        db.transition_task(
            task_id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            task_id,
            TaskState::Confirmed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            task_id,
            TaskState::Verifying,
            Actor::ConnectedAi,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            task_id,
            TaskState::PreviewReady,
            Actor::ConnectedAi,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            task_id,
            TaskState::AwaitingUserAcceptance,
            Actor::Nova,
            None,
            None,
            false,
            None,
        )
        .unwrap();
    }

    /// WP2A fixture helper: satisfies every piece of `evaluate_completion_gate`
    /// except user acceptance itself — a succeeded dev action for `round`,
    /// passing test+build evidence bound to that action's attempt number,
    /// and a valid/complete self-check whose `requirement_coverage` is an
    /// exact match for the task's confirmed requirement items (as produced
    /// by `list_confirmed_requirement_items`, i.e. whatever non-empty
    /// fields the caller's `CreateTaskRequest` actually set). Callers still
    /// need to call `db.record_user_acceptance(...)` themselves for the
    /// relevant `result_generation` before a `Completed` transition can
    /// succeed — this only clears the evidence/self-check half of the gate.
    fn satisfy_completion_gate_evidence_and_self_check(
        db: &Database,
        project_id: &str,
        task_id: &str,
        generation: i64,
        round: i64,
    ) {
        use crate::storage::evidence::{RecordBuildEvidenceRequest, RecordTestEvidenceRequest};
        use crate::storage::self_check::RecordSelfCheckRequest;

        let dev_key = format!("dev-{task_id}-round-{round}");
        let planned = db.plan_action(task_id, "ai_development", &dev_key).unwrap();
        let started = db
            .start_action(task_id, &dev_key, false)
            .unwrap_or(planned);
        let attempt = started.attempt_number;
        db.finish_action(
            task_id,
            &dev_key,
            ActionStatus::Succeeded,
            None,
            None,
            attempt,
        )
        .unwrap();

        db.record_test_evidence(&RecordTestEvidenceRequest {
            project_id,
            task_id,
            result_generation: generation,
            round,
            result_attempt: attempt,
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
        db.record_build_evidence(&RecordBuildEvidenceRequest {
            project_id,
            task_id,
            result_generation: generation,
            round,
            result_attempt: attempt,
            build_command_redacted: Some("cargo build"),
            status: "passed",
            authorization_source: None,
            exit_code: Some(0),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();

        let items = db.list_confirmed_requirement_items(task_id).unwrap();
        let coverage: Vec<serde_json::Value> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "item": format!("{}:{}", item.field, item.item_index),
                    "status": "covered",
                })
            })
            .collect();
        db.record_self_check(&RecordSelfCheckRequest {
            project_id,
            task_id,
            round,
            result_attempt: attempt,
            parse_status: "valid",
            self_reported_status: Some("self_reported_complete"),
            requirement_coverage_json: Some(&serde_json::to_string(&coverage).unwrap()),
            check_results_json: Some("[]"),
            unfinished_items_json: Some("[]"),
            blocking_issues_json: Some("[]"),
            raw_block_redacted: "mock self-check",
        })
        .unwrap();
    }

    #[test]
    fn completed_requires_a_prior_user_acceptance_record() {
        let dir = temp_dir("completed-needs-acceptance");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        drive_to_awaiting_acceptance(&db, &task.id);

        let without_acceptance = db.transition_task(
            &task.id,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        );
        assert!(
            without_acceptance.is_err(),
            "completed must be blocked without a user_acceptance row"
        );

        satisfy_completion_gate_evidence_and_self_check(&db, &project_id, &task.id, 1, 1);
        db.record_user_acceptance(&project_id, &task.id, true, None)
            .unwrap();
        let with_acceptance = db.transition_task(
            &task.id,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        );
        assert!(with_acceptance.is_ok());
        assert_eq!(with_acceptance.unwrap().state, TaskState::Completed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejecting_the_current_result_does_not_allow_completion() {
        let dir = temp_dir("rejected-result");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        drive_to_awaiting_acceptance(&db, &task.id);
        db.record_user_acceptance(&project_id, &task.id, false, Some("需要修改"))
            .unwrap();

        let result = db.transition_task(
            &task.id,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        );
        assert!(
            result.is_err(),
            "an explicit rejection must not unlock completion"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn acceptance_can_only_be_recorded_while_awaiting_user_acceptance() {
        let dir = temp_dir("acceptance-state-gate");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        assert!(
            db.record_user_acceptance(&project_id, &task.id, true, None)
                .is_err(),
            "a draft task has never reached awaiting_user_acceptance"
        );

        db.transition_task(
            &task.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        assert!(
            db.record_user_acceptance(&project_id, &task.id, true, None)
                .is_err(),
            "awaiting_confirmation is not awaiting_user_acceptance"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_result_generation_invalidates_the_old_accepted_generation() {
        let dir = temp_dir("new-generation-invalidates-old");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        drive_to_awaiting_acceptance(&db, &task.id);
        assert_eq!(db.task(&task.id).unwrap().result_generation, 1);

        db.record_user_acceptance(&project_id, &task.id, true, None)
            .unwrap();

        // The user asks for one more change: a fresh result is produced,
        // which must invalidate the generation-1 acceptance.
        db.transition_task(
            &task.id,
            TaskState::SafeFixing,
            Actor::User,
            Some("再改一下"),
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Verifying,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::PreviewReady,
            Actor::ConnectedAi,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            db.task(&task.id).unwrap().result_generation,
            2,
            "a new result bumps the generation"
        );
        db.transition_task(
            &task.id,
            TaskState::AwaitingUserAcceptance,
            Actor::Nova,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let stale_completion = db.transition_task(
            &task.id,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        );
        assert!(
            stale_completion.is_err(),
            "the generation-1 acceptance must not be able to complete generation 2"
        );

        satisfy_completion_gate_evidence_and_self_check(&db, &project_id, &task.id, 2, 1);
        db.record_user_acceptance(&project_id, &task.id, true, None)
            .unwrap();
        let completed = db
            .transition_task(
                &task.id,
                TaskState::Completed,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        assert_eq!(completed.state, TaskState::Completed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrated_generation_zero_acceptance_cannot_complete_a_newer_generation() {
        let dir = temp_dir("legacy-generation-zero");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        // Simulate exactly what the v2 -> v3 migration produces for a
        // pre-remediation acceptance row: `result_generation = 0`,
        // inserted directly (not through `record_user_acceptance`, which
        // would refuse it — this row predates that gate).
        db.conn()
            .execute(
                "INSERT INTO user_acceptance (id, project_id, task_id, result_generation, accepted, user_note, accepted_at, created_at)
                 VALUES (?1, ?2, ?3, 0, 1, NULL, ?4, ?4)",
                rusqlite::params![new_id(), project_id, task.id, "2026-01-01T00:00:00Z"],
            )
            .unwrap();

        drive_to_awaiting_acceptance(&db, &task.id);
        assert_eq!(db.task(&task.id).unwrap().result_generation, 1);

        let result = db.transition_task(
            &task.id,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        );
        assert!(
            result.is_err(),
            "a migrated generation-0 acceptance must never authorize completion of a real (generation >= 1) result"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_and_cancelled_can_never_reach_completed() {
        let dir = temp_dir("failed-cancelled-terminal");
        let (db, project_id) = db_with_project(&dir);

        let failing_task = new_task(&db, &project_id);
        db.transition_task(
            &failing_task.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &failing_task.id,
            TaskState::Confirmed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &failing_task.id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &failing_task.id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &failing_task.id,
            TaskState::Failed,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        // A task that's already `failed` isn't `awaiting_user_acceptance`,
        // so it can't even have an acceptance recorded against it anymore
        // — the completion gate is doubly closed.
        assert!(
            db.record_user_acceptance(&project_id, &failing_task.id, true, None)
                .is_err(),
            "acceptance must be refused once the task has left awaiting_user_acceptance"
        );
        assert!(db
            .transition_task(
                &failing_task.id,
                TaskState::Completed,
                Actor::User,
                None,
                None,
                false,
                None
            )
            .is_err());

        let cancelled_task = new_task(&db, &project_id);
        db.transition_task(
            &cancelled_task.id,
            TaskState::Cancelled,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        assert!(
            db.record_user_acceptance(&project_id, &cancelled_task.id, true, None)
                .is_err(),
            "acceptance must be refused for a cancelled task"
        );
        assert!(db
            .transition_task(
                &cancelled_task.id,
                TaskState::Completed,
                Actor::User,
                None,
                None,
                false,
                None
            )
            .is_err());
        assert!(
            db.transition_task(
                &cancelled_task.id,
                TaskState::Running,
                Actor::System,
                None,
                None,
                false,
                None
            )
            .is_err(),
            "cancelled must never auto-recover to running"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn interrupted_recovery_requires_judgement_not_a_direct_hop_to_running() {
        let dir = temp_dir("interrupted-recovery");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        db.transition_task(
            &task.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Confirmed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Interrupted,
            Actor::System,
            Some("应用重启"),
            None,
            true,
            Some("检测到中断"),
        )
        .unwrap();

        assert!(
            db.transition_task(
                &task.id,
                TaskState::Running,
                Actor::System,
                None,
                None,
                false,
                None
            )
            .is_err(),
            "interrupted must not be able to hop directly back into running"
        );
        let resumed = db
            .transition_task(
                &task.id,
                TaskState::Queued,
                Actor::User,
                Some("用户选择继续"),
                None,
                false,
                None,
            )
            .unwrap();
        assert_eq!(resumed.state, TaskState::Queued);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn automatic_fix_cycle_is_capped_and_forces_needs_user_action() {
        let dir = temp_dir("fix-cycle-cap");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        db.transition_task(
            &task.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Confirmed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        for _ in 0..MAX_AUTO_FIX_CYCLES {
            db.transition_task(
                &task.id,
                TaskState::Verifying,
                Actor::ConnectedAi,
                None,
                None,
                false,
                None,
            )
            .unwrap();
            db.transition_task(
                &task.id,
                TaskState::SafeFixing,
                Actor::ConnectedAi,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        }
        db.transition_task(
            &task.id,
            TaskState::Verifying,
            Actor::ConnectedAi,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let blocked = db.transition_task(
            &task.id,
            TaskState::SafeFixing,
            Actor::ConnectedAi,
            None,
            None,
            false,
            None,
        );
        assert!(blocked.is_err(), "the automatic fix cycle must be capped");

        let redirected = db
            .transition_task(
                &task.id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some("修复次数已达上限"),
                None,
                false,
                None,
            )
            .unwrap();
        assert_eq!(redirected.state, TaskState::NeedsUserAction);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_user_initiated_fix_is_not_limited_by_the_automatic_cycle_cap() {
        let dir = temp_dir("user-fix-not-capped");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        db.transition_task(
            &task.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Confirmed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        for _ in 0..(MAX_AUTO_FIX_CYCLES + 2) {
            db.transition_task(
                &task.id,
                TaskState::Verifying,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
            db.transition_task(
                &task.id,
                TaskState::SafeFixing,
                Actor::User,
                Some("用户要求再修一次"),
                None,
                false,
                None,
            )
            .unwrap();
        }
        let task = db.task(&task.id).unwrap();
        assert_eq!(task.state, TaskState::SafeFixing);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scope_change_after_preview_does_not_silently_mutate_the_same_task() {
        // A scope change must become a new task/re-confirmation, not an
        // edge that lets `preview_ready` jump straight into `running` with
        // different scope.
        assert!(!is_legal_transition(
            TaskState::PreviewReady,
            TaskState::Running
        ));
        assert!(!is_legal_transition(
            TaskState::AwaitingUserAcceptance,
            TaskState::Running
        ));
    }

    #[test]
    fn state_and_event_are_written_in_the_same_transaction() {
        let dir = temp_dir("same-transaction");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        let before_events = db
            .list_task_state_events(&project_id, &task.id)
            .unwrap()
            .len();

        db.transition_task(
            &task.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            Some("提交确认"),
            Some("corr-1"),
            false,
            None,
        )
        .unwrap();

        let after = db.task(&task.id).unwrap();
        let events = db.list_task_state_events(&project_id, &task.id).unwrap();
        assert_eq!(after.state, TaskState::AwaitingConfirmation);
        assert_eq!(events.len(), before_events + 1);
        assert_eq!(
            events.last().unwrap().correlation_id.as_deref(),
            Some("corr-1")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn transition_reason_is_redacted_before_it_reaches_task_state_events() {
        let dir = temp_dir("redact-transition-reason");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        db.transition_task(
            &task.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            Some("token=abc123supersecretvalue"),
            None,
            false,
            None,
        )
        .unwrap();

        let events = db.list_task_state_events(&project_id, &task.id).unwrap();
        let reason = events.last().unwrap().reason.as_deref().unwrap();
        assert!(
            !reason.contains("abc123supersecretvalue"),
            "the reason text must be redacted before it is ever persisted: {reason}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Backend-internal code (recovery, system-driven transitions) must
    /// keep working with non-`user` actors — only the Tauri command
    /// boundary is restricted to `Actor::User`, not `Database::transition_task`
    /// itself.
    #[test]
    fn internal_callers_can_still_use_system_and_recovery_actors() {
        let dir = temp_dir("internal-actors-still-work");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        let result = db.transition_task(
            &task.id,
            TaskState::AwaitingConfirmation,
            Actor::System,
            Some("系统自动提交"),
            None,
            true,
            Some("恢复流程"),
        );
        assert!(
            result.is_ok(),
            "backend-internal Actor::System + is_recovery must still be usable"
        );
        let events = db.list_task_state_events(&project_id, &task.id).unwrap();
        assert_eq!(events.last().unwrap().actor, Actor::System);
        assert!(events.last().unwrap().is_recovery);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_transaction_failure_leaves_both_state_and_event_unchanged() {
        let dir = temp_dir("transaction-failure");
        let (db, project_id) = db_with_project(&dir);
        let task = new_task(&db, &project_id);
        let before = db.task(&task.id).unwrap();
        let before_events = db.list_task_state_events(&project_id, &task.id).unwrap();

        // An illegal transition is rejected before any write is attempted,
        // so this also proves the "reject" path never partially applies.
        let result = db.transition_task(
            &task.id,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        );
        assert!(result.is_err());

        let after = db.task(&task.id).unwrap();
        let after_events = db.list_task_state_events(&project_id, &task.id).unwrap();
        assert_eq!(before, after);
        assert_eq!(before_events, after_events);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unfinished_task_for_project_ignores_terminal_states() {
        let dir = temp_dir("unfinished-task");
        let (db, project_id) = db_with_project(&dir);
        assert!(db
            .unfinished_task_for_project(&project_id)
            .unwrap()
            .is_none());

        let task = new_task(&db, &project_id);
        let found = db
            .unfinished_task_for_project(&project_id)
            .unwrap()
            .unwrap();
        assert_eq!(found.id, task.id);

        db.transition_task(
            &task.id,
            TaskState::Cancelled,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        assert!(db
            .unfinished_task_for_project(&project_id)
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
