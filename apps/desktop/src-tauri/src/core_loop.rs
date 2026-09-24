//! Phase 3: Open Local Project Dual-AI Core Loop.
//!
//! The service layer driving one confirmed task through the fixed
//! product-spec loop — requirement (already confirmed by the time this
//! runs) -> developer AI -> independent evidence collection -> read-only
//! reviewer -> at most one fix pass -> second review -> user acceptance —
//! entirely on top of already-accepted Phase 1/2 machinery:
//!
//! - State transitions go exclusively through
//!   [`crate::db::Database::transition_task`] (Phase 2's CAS-guarded state
//!   machine); this module never writes `tasks.state` directly.
//! - Every potentially-destructive step (a developer AI run) and every
//!   read-only-but-still-worth-tracking step (a reviewer run) is gated by
//!   the Phase 2 Action Ledger (`plan_action`/`start_action`/
//!   `finish_action`), so a crash mid-run can never be silently re-run.
//! - Developer/reviewer processes are invoked only through Phase 1's safe
//!   process boundary (`runtime::process`, via `runtime::ai_adapter`) —
//!   argument-array execution, no shell string, timeout, cancellation, and
//!   redaction all reused unchanged.
//! - Evidence (git diff/status, the invocation itself) is recorded through
//!   Phase 2's evidence repository, independent of anything the AI claims.
//!
//! Tauri commands are a thin wrapper around [`run_core_loop`] — see
//! `commands::start_core_loop` — this module contains all the actual
//! business logic, per the architecture requirement that commands stay a
//! permission/parameter boundary only.

pub(crate) mod documents;
mod review_isolation;
pub(crate) mod test_runner;

#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::db::Database;
use crate::error::{NovaError, NovaResult};
use crate::models::{ActionStatus, Actor, Task, TaskState};
use crate::runtime::ai_adapter::{
    self, DevReviewAdapter, DevelopmentRequest, ReviewRequest, ReviewVerdict,
};
use crate::runtime::process;
use crate::runtime::types::{
    ExecutionErrorKind, ExecutionOutcome, ExecutionRequest, ExecutionResult, ToolKind,
};
use crate::security::redact_sensitive_text;

const DEV_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const REVIEW_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const TEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Outcome of a single review round (first or second/fix). Distinguishing
/// `IntegrityViolation` from `NeedsFix` matters: a normal `CHANGES_REQUIRED`
/// verdict is expected product behavior and drives the one allowed
/// automatic fix pass; a detected read-only violation is a trust/safety
/// event that must stop the loop for a human, never be treated as routine.
enum RoundOutcome {
    Approved,
    NeedsFix(String),
    IntegrityViolation,
    /// A required Action Ledger step was `uncertain` (crash-recovery
    /// scenario) and the round could not proceed automatically; the task
    /// has already been moved to `needs_user_action`.
    ActionUncertain,
    /// The developer or reviewer AI process itself failed to complete or
    /// was cancelled (not a review verdict) — the task has already been
    /// transitioned to `failed` or `cancelled` directly (see
    /// [`was_cancelled`]); the caller must take no further action.
    Stopped,
}

pub struct CoreLoopOutcome {
    pub task: Task,
}

/// v0.1.0 scope correction (`CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`,
/// Blocker 2): the real v0.1.0 product main path — **exactly one** selected
/// developer AI (Codex or Claude Code, the user's choice), Nova's own
/// independent verification of the real file/Git change and test/build
/// result, at most one automatic fix pass by that *same* developer AI when
/// verification fails, then `preview_ready`/`awaiting_user_acceptance`.
/// There is no reviewer step anywhere in this function — a second AI is
/// never invoked, never probed for readiness, and never required to be
/// installed, trusted, or logged in for this to run to completion. This is
/// the only entry point `commands::start_core_loop` calls; the dormant
/// v0.2.0 dual-AI orchestration lives in [`run_core_loop_dual_ai_v0_2`]
/// below, unused by any command.
///
/// Drives `task_id` through the loop until it reaches a stopping point:
/// `awaiting_user_acceptance` (success), `failed` (second verification
/// attempt still failing), `needs_user_action` (dirty worktree or an
/// uncertain action needing user input), or `cancelled`. Returns the task's
/// final state, whatever it is — this function never returns `Err` for a
/// legitimate product-level stop, only for a genuine programming/
/// precondition error (wrong starting state, missing project, bad
/// `ai_kind`).
pub fn run_core_loop(
    db: &Database,
    task_id: &str,
    adapter: &dyn DevReviewAdapter,
    cancel: &CancellationToken,
    app_data_dir: &Path,
) -> NovaResult<CoreLoopOutcome> {
    let task = db.task(task_id)?;
    if !matches!(task.state, TaskState::Confirmed | TaskState::Queued) {
        return Err(NovaError::Config(format!(
            "只能从 confirmed 或 queued 状态启动核心循环，当前状态：{}",
            task.state
        )));
    }
    let project = db.project(&task.project_id)?;
    let project_dir = PathBuf::from(&project.local_path);

    let dev_kind = ai_adapter::ai_kind_from_str(task.ai_kind.as_deref().unwrap_or(""))
        .ok_or_else(|| NovaError::Config("任务未设置有效的开发 AI 类型（ai_kind）".to_string()))?;

    if task.state == TaskState::Confirmed {
        db.transition_task(
            task_id,
            TaskState::Queued,
            Actor::System,
            Some("核心循环已启动"),
            None,
            false,
            None,
        )?;
    }

    if cancel.is_cancelled() {
        db.transition_task(
            task_id,
            TaskState::Cancelled,
            Actor::User,
            Some("用户在开始前取消"),
            None,
            false,
            None,
        )?;
        documents::generate_result_documents(db, &project.id, task_id, app_data_dir)?;
        return Ok(CoreLoopOutcome {
            task: db.task(task_id)?,
        });
    }

    db.transition_task(
        task_id,
        TaskState::Running,
        Actor::System,
        Some("开发 AI 开始工作"),
        None,
        false,
        None,
    )?;

    // Same dirty-worktree guard as the dormant v0.2.0 path, reusing the
    // same already-accepted Phase 2 transition edge (`running ->
    // needs_user_action`) — no AI has been invoked yet, so this still
    // fully protects the user's uncommitted work before any generated
    // change.
    if let Some(status) = git_status_porcelain(&project_dir, cancel)? {
        if !status.trim().is_empty() {
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some("项目工作区存在未提交的更改，需要用户先提交或清理后再开始"),
                None,
                false,
                Some("dirty worktree guard blocked task start"),
            )?;
            documents::generate_result_documents(db, &project.id, task_id, app_data_dir)?;
            return Ok(CoreLoopOutcome {
                task: db.task(task_id)?,
            });
        }
    }

    // Codex re-review remediation (Major 1, resume follow-through): if a
    // prior invocation of this function already ran round 1 to a genuine
    // `NeedsFix` conclusion and started round 2 — proven by a real round-2
    // Action Ledger entry existing at all, exactly what
    // `current_pending_round` now checks — resume directly at round 2.
    // Re-running round 1's `run_single_ai_round` here would immediately hit
    // `gate_action` on round 1's own already-terminal (non-`succeeded`)
    // test/build action and incorrectly re-stall the task on an issue round
    // 1 already resolved by handing off to the one automatic fix pass, even
    // though the *real* stuck point (round 2) was what the user just
    // unblocked via `unblock_test_execution_for_retry`. `prior_findings` is
    // `None` here (not round 1's original failure text) because round 2's
    // own developer-AI step has, by construction, already run (a round-2
    // Action Ledger entry cannot exist otherwise) — this resume path only
    // ever reaches `write_task_package` again in the narrow crash-recovery
    // case where round 2's *own* dev action is still `planned`, which does
    // not need round 1's original findings either.
    if current_pending_round(db, &project.id, task_id)? == 2 {
        let round2 = run_single_ai_round(
            db,
            task_id,
            &project.id,
            &project_dir,
            dev_kind,
            adapter,
            cancel,
            2,
            None,
            app_data_dir,
        )?;
        match round2 {
            SingleAiRoundOutcome::Verified => finish_success_path(db, &project.id, task_id, 2)?,
            SingleAiRoundOutcome::NeedsFix(_) => {
                db.transition_task(
                    task_id,
                    TaskState::Failed,
                    Actor::System,
                    Some("修复后 Nova 独立验证仍未通过，尚未完工，由用户决定是否开始下一轮"),
                    None,
                    false,
                    None,
                )?;
            }
            SingleAiRoundOutcome::ActionUncertain | SingleAiRoundOutcome::Stopped => {}
        }
        documents::generate_result_documents(db, &project.id, task_id, app_data_dir)?;
        return Ok(CoreLoopOutcome {
            task: db.task(task_id)?,
        });
    }

    let round1 = run_single_ai_round(
        db,
        task_id,
        &project.id,
        &project_dir,
        dev_kind,
        adapter,
        cancel,
        1,
        None,
        app_data_dir,
    )?;
    match round1 {
        SingleAiRoundOutcome::Verified => finish_success_path(db, &project.id, task_id, 1)?,
        SingleAiRoundOutcome::NeedsFix(findings) => {
            db.transition_task(
                task_id,
                TaskState::SafeFixing,
                Actor::System,
                Some("Nova 独立验证未通过，开始本任务唯一一次自动修复"),
                None,
                false,
                None,
            )?;
            let round2 = run_single_ai_round(
                db,
                task_id,
                &project.id,
                &project_dir,
                dev_kind,
                adapter,
                cancel,
                2,
                Some(&findings),
                app_data_dir,
            )?;
            match round2 {
                SingleAiRoundOutcome::Verified => finish_success_path(db, &project.id, task_id, 2)?,
                SingleAiRoundOutcome::NeedsFix(_) => {
                    db.transition_task(
                        task_id,
                        TaskState::Failed,
                        Actor::System,
                        Some("修复后 Nova 独立验证仍未通过，尚未完工，由用户决定是否开始下一轮"),
                        None,
                        false,
                        None,
                    )?;
                }
                SingleAiRoundOutcome::ActionUncertain | SingleAiRoundOutcome::Stopped => {}
            }
        }
        SingleAiRoundOutcome::ActionUncertain | SingleAiRoundOutcome::Stopped => {}
    }

    documents::generate_result_documents(db, &project.id, task_id, app_data_dir)?;
    Ok(CoreLoopOutcome {
        task: db.task(task_id)?,
    })
}

/// v0.1.0 scope correction (`CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`):
/// the dormant v0.2.0 dual-AI orchestration — developer AI -> independent
/// evidence -> read-only reviewer -> at most one fix pass -> second review
/// -> user acceptance. Preserved verbatim (not deleted, not weakened) for
/// forward compatibility and for this module's own dual-AI test coverage,
/// but **no Tauri command calls this** — `commands::start_core_loop` calls
/// [`run_core_loop`] (the single-AI v0.1.0 path) below instead. Do not wire
/// this back into any command without also re-establishing the v0.2.0
/// product gating this audit explicitly deferred (mandatory second AI,
/// role swap, FIRST/SECOND review as required artifacts).
///
/// Drives `task_id` through the loop until it reaches a stopping point:
/// `awaiting_user_acceptance` (success), `failed` (second review still
/// rejected), `needs_user_action` (dirty worktree, uncertain action, or a
/// reviewer integrity violation), or `cancelled`. Returns the task's final
/// state, whatever it is — this function never returns `Err` for a
/// legitimate product-level stop, only for a genuine programming/precondition
/// error (wrong starting state, missing project, bad `ai_kind`).
///
/// Not `#[cfg(test)]`-gated on purpose: a real v0.2.0 build must be able to
/// wire this back into a command without first having to resurrect
/// deleted/stripped code — only *this crate's own* test suite currently
/// calls it (`#[allow(dead_code)]` covers the resulting "never constructed
/// outside tests" warning in a non-test build), which is exactly the "kept
/// dormant, not in the default path" shape the scope-correction audit asked
/// for.
#[allow(dead_code)]
pub(crate) fn run_core_loop_dual_ai_v0_2(
    db: &Database,
    task_id: &str,
    adapter: &dyn DevReviewAdapter,
    cancel: &CancellationToken,
    app_data_dir: &Path,
) -> NovaResult<CoreLoopOutcome> {
    let task = db.task(task_id)?;
    if !matches!(task.state, TaskState::Confirmed | TaskState::Queued) {
        return Err(NovaError::Config(format!(
            "只能从 confirmed 或 queued 状态启动核心循环，当前状态：{}",
            task.state
        )));
    }
    let project = db.project(&task.project_id)?;
    let project_dir = PathBuf::from(&project.local_path);

    let dev_kind = ai_adapter::ai_kind_from_str(task.ai_kind.as_deref().unwrap_or(""))
        .ok_or_else(|| NovaError::Config("任务未设置有效的开发 AI 类型（ai_kind）".to_string()))?;
    let reviewer_kind = ai_adapter::the_other(dev_kind);

    if task.state == TaskState::Confirmed {
        db.transition_task(
            task_id,
            TaskState::Queued,
            Actor::System,
            Some("核心循环已启动"),
            None,
            false,
            None,
        )?;
    }

    if cancel.is_cancelled() {
        db.transition_task(
            task_id,
            TaskState::Cancelled,
            Actor::User,
            Some("用户在开始前取消"),
            None,
            false,
            None,
        )?;
        documents::generate_result_documents(db, &project.id, task_id, app_data_dir)?;
        return Ok(CoreLoopOutcome {
            task: db.task(task_id)?,
        });
    }

    db.transition_task(
        task_id,
        TaskState::Running,
        Actor::System,
        Some("开发 AI 开始工作"),
        None,
        false,
        None,
    )?;

    // The dirty-worktree guard runs once the task is `running` (not
    // `queued`/`confirmed`) specifically because `needs_user_action` is
    // only a legal destination from `running` in the Phase 2 state
    // machine — reusing that already-accepted transition table exactly as
    // it stands, rather than adding a new edge to it, per the "do not
    // modify the already-accepted Phase 2 baseline" constraint. No AI has
    // been invoked yet at this point, so this still fully protects the
    // user's uncommitted work before any generated change.
    if let Some(status) = git_status_porcelain(&project_dir, cancel)? {
        if !status.trim().is_empty() {
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some("项目工作区存在未提交的更改，需要用户先提交或清理后再开始"),
                None,
                false,
                Some("dirty worktree guard blocked task start"),
            )?;
            documents::generate_result_documents(db, &project.id, task_id, app_data_dir)?;
            return Ok(CoreLoopOutcome {
                task: db.task(task_id)?,
            });
        }
    }

    let round1 = run_one_round(
        db,
        task_id,
        &project.id,
        &project_dir,
        dev_kind,
        reviewer_kind,
        adapter,
        cancel,
        1,
        None,
        app_data_dir,
    )?;
    match round1 {
        RoundOutcome::Approved => finish_success_path(db, &project.id, task_id, 1)?,
        RoundOutcome::NeedsFix(findings) => {
            db.transition_task(
                task_id,
                TaskState::SafeFixing,
                Actor::System,
                Some("首次审核未通过，开始本任务唯一一次自动修复"),
                None,
                false,
                None,
            )?;
            let round2 = run_one_round(
                db,
                task_id,
                &project.id,
                &project_dir,
                dev_kind,
                reviewer_kind,
                adapter,
                cancel,
                2,
                Some(&findings),
                app_data_dir,
            )?;
            match round2 {
                RoundOutcome::Approved => finish_success_path(db, &project.id, task_id, 2)?,
                RoundOutcome::NeedsFix(_) => {
                    db.transition_task(
                        task_id,
                        TaskState::Failed,
                        Actor::System,
                        Some("二次审核仍未通过，尚未完工，由用户决定是否开始下一轮"),
                        None,
                        false,
                        None,
                    )?;
                }
                RoundOutcome::IntegrityViolation => mark_integrity_violation(db, task_id)?,
                RoundOutcome::ActionUncertain | RoundOutcome::Stopped => {}
            }
        }
        RoundOutcome::IntegrityViolation => mark_integrity_violation(db, task_id)?,
        RoundOutcome::ActionUncertain | RoundOutcome::Stopped => {}
    }

    documents::generate_result_documents(db, &project.id, task_id, app_data_dir)?;
    Ok(CoreLoopOutcome {
        task: db.task(task_id)?,
    })
}

fn mark_integrity_violation(db: &Database, task_id: &str) -> NovaResult<()> {
    db.transition_task(
        task_id,
        TaskState::NeedsUserAction,
        Actor::System,
        Some("审核 AI 的只读技术核查未通过，需要用户判断"),
        None,
        false,
        Some("reviewer read-only integrity violation"),
    )?;
    Ok(())
}

/// Blocker 3 defense-in-depth: even though `run_tests_for_round` already
/// structurally prevents a `NeedsFix`/`Failed` test outcome from ever
/// reaching `RoundOutcome::Approved` in the first place, this re-verifies
/// directly against the database that the *current* round has a
/// `passed`/`no_tests_applicable` test-evidence row before allowing
/// `preview_ready` — so a future code change that adds another path to
/// `Approved` can never silently skip the test gate. A reviewer's `approve`
/// verdict alone is never sufficient.
fn finish_success_path(
    db: &Database,
    project_id: &str,
    task_id: &str,
    round: i64,
) -> NovaResult<()> {
    let result_generation = db.task(task_id)?.result_generation;
    let evidence =
        db.list_test_evidence_for_round(project_id, task_id, result_generation, round)?;
    let test_gate_ok = evidence
        .iter()
        .any(|e| e.status == "passed" || e.status == "no_tests_applicable");
    if !test_gate_ok {
        return Err(NovaError::Config(
            "缺少当前轮次的独立测试证据（通过或不适用），结果不能进入验收".to_string(),
        ));
    }

    // Codex re-review remediation (Major 3): symmetric defense-in-depth
    // check alongside the test gate above — a round can never reach
    // `preview_ready` without its own independent build evidence either
    // (passed or genuinely not applicable), never merely because the
    // caller happened to skip the build step.
    let build_evidence =
        db.list_build_evidence_for_round(project_id, task_id, result_generation, round)?;
    let build_gate_ok = build_evidence
        .iter()
        .any(|e| e.status == "passed" || e.status == "no_build_applicable");
    if !build_gate_ok {
        return Err(NovaError::Config(
            "缺少当前轮次的独立构建证据（通过或不适用），结果不能进入验收".to_string(),
        ));
    }

    db.transition_task(
        task_id,
        TaskState::PreviewReady,
        Actor::System,
        Some("Nova 独立验证通过，结果已就绪"),
        None,
        false,
        None,
    )?;
    db.transition_task(
        task_id,
        TaskState::AwaitingUserAcceptance,
        Actor::Nova,
        Some("等待用户确认最终效果"),
        None,
        false,
        None,
    )?;
    Ok(())
}

/// Second re-review remediation (Major 5): the single atomic business entry
/// point for a user's accept/reject decision on a task's current result.
/// `commands::accept_task_result` is a thin Tauri-parameter-boundary
/// wrapper around this — kept here, not in `commands.rs`, so it can be
/// exercised directly by this module's own tests without a `State<AppState>`.
///
/// Records the acceptance/rejection (bound to the task's *current*
/// `result_generation` by `Database::record_user_acceptance` itself),
/// transitions to `completed` (accepted) or `needs_user_action` (rejected —
/// the state machine's legal "stopped, needs a decision" edge from
/// `awaiting_user_acceptance`), then regenerates the seven documents so
/// `FINAL_RESULT.md` reflects whichever actually happened instead of
/// staying stuck on "awaiting acceptance" forever. A document-regeneration
/// failure never undoes the already-successful database write — it only
/// sets `tasks.document_sync_pending`, which `resync_task_documents`
/// (or this function's own next call for the same task) can clear later.
pub fn accept_task_result(
    db: &Database,
    project_id: &str,
    task_id: &str,
    accepted: bool,
    note: Option<&str>,
    app_data_dir: &Path,
) -> NovaResult<Task> {
    let task = db.task(task_id)?;
    if task.project_id != project_id {
        return Err(NovaError::Config(
            "task_id 与 project_id 不匹配，已拒绝".to_string(),
        ));
    }
    let target_state = if accepted {
        TaskState::Completed
    } else {
        TaskState::NeedsUserAction
    };
    db.record_user_acceptance(project_id, task_id, accepted, note)?;
    db.transition_task(task_id, target_state, Actor::User, note, None, false, None)?;

    match documents::generate_result_documents(db, project_id, task_id, app_data_dir) {
        Ok(()) => db.set_document_sync_pending(task_id, false)?,
        Err(_) => {
            // Best-effort marker only: the accept/reject decision above
            // already committed and must stand regardless of whether this
            // secondary write also succeeds.
            let _ = db.set_document_sync_pending(task_id, true);
        }
    }

    db.task(task_id)
}

/// Retries seven-document regeneration for a task whose
/// `document_sync_pending` flag is set — lets the UI (or a future
/// app-startup sweep) retry without requiring another accept/reject
/// decision.
pub fn resync_task_documents(
    db: &Database,
    project_id: &str,
    task_id: &str,
    app_data_dir: &Path,
) -> NovaResult<Task> {
    documents::generate_result_documents(db, project_id, task_id, app_data_dir)?;
    db.set_document_sync_pending(task_id, false)?;
    db.task(task_id)
}

/// Codex re-review remediation (Major 1): the round a task is currently
/// stuck on if it is sitting in `needs_user_action` because its
/// `dev`/`test`-`{task_id}-round-N` action needed input the user did not
/// yet have (no detectable test command, a rejected allowlist check, ...)
/// — round 1 unless *any* round-2 Action Ledger entry already exists for
/// this task, in which case it is the round-2 automatic fix pass. Only
/// ever 1 or 2, matching the fixed "at most one automatic fix pass" product
/// rule.
///
/// Previously derived from whether a round-1 `task_reviews` row existed —
/// correct for the dormant v0.2.0 dual-AI path (which always writes one),
/// but the v0.1.0 single-AI path never writes `task_reviews` at all, so
/// this always returned `1`, even for a task genuinely stuck in its round-2
/// fix pass. The Action Ledger is the single source of truth both paths
/// share unconditionally: `gate_action` always plans a round's `dev-...`
/// action *before* anything else happens for that round (see
/// `run_dev_and_test_round`), so a round-2 `dev`/`test` action existing at
/// all is itself sufficient, real proof round 2 has begun. Deliberately
/// does *not* check a `build-...` key: since Codex re-review Major 3, the
/// build action key is generation-aware
/// (`build-{task_id}-generation-{result_generation}-round-{round}`, see
/// [`build_action_key`]) and this function has no generation to plug in —
/// but that is not a gap, because `dev`/`test` for round 2 are *always*
/// planned strictly before build ever runs (see `run_dev_and_test_round`),
/// so checking them alone is already a complete, sufficient signal. Never
/// inspects the free-text `reason` a state transition carries; that text
/// is for humans, not for control flow.
pub fn current_pending_round(db: &Database, project_id: &str, task_id: &str) -> NovaResult<i64> {
    let task = db.task(task_id)?;
    if task.project_id != project_id {
        return Err(NovaError::Config(
            "task_id 与 project_id 不匹配，已拒绝".to_string(),
        ));
    }
    let round2_dev_key = format!("dev-{task_id}-round-2");
    if db.find_action(task_id, &round2_dev_key)?.is_some() {
        return Ok(2);
    }
    // Work Package 3 remediation: the test key is generation-aware (see
    // [`test_action_key`]), so round 2's detection must look it up under
    // the task's *current* `result_generation` — checking the dev key
    // above alone would already be sufficient (`run_dev_and_test_round`
    // always plans a round's `dev-...` action strictly before its
    // `test-...` action), but this keeps both signals consistent and
    // auditable rather than leaving a check that can never match anything.
    let round2_test_key = test_action_key(task_id, task.result_generation, 2);
    if db.find_action(task_id, &round2_test_key)?.is_some() {
        return Ok(2);
    }
    Ok(1)
}

/// Second re-review remediation (Blocker 3): after the user supplies what
/// a stalled generation-aware test action (see [`test_action_key`]) was
/// waiting on (a real no-tests confirmation, or a corrected/allowlisted
/// test command), this
/// resets that exact action back to `planned` and, if the task is
/// currently `needs_user_action`, returns it to `queued` — the same legal
/// state `start_core_loop` already accepts — so the frontend's next
/// "开始任务" actually re-attempts the round instead of the loop staying
/// permanently stuck (a `failed`, non-`uncertain` action would otherwise
/// never become startable again).
/// Codex re-review remediation (Major 2 + Major 3): also resets the
/// *current* pending round's generation-aware build action
/// (`build-{task_id}-generation-{result_generation}-round-{round}`) so a
/// task stalled specifically on build detection/allowlist/evidence — not
/// just a stalled test action — can also be resumed. `reset_failed_action_
/// for_retry`'s own `WHERE status = 'failed'` condition makes this a safe
/// no-op for whichever of the two (test or build) was not actually the one
/// that stalled.
pub fn unblock_test_execution_for_retry(
    db: &Database,
    project_id: &str,
    task_id: &str,
) -> NovaResult<Task> {
    let round = current_pending_round(db, project_id, task_id)?;
    let result_generation = db.task(task_id)?.result_generation;
    let test_key = test_action_key(task_id, result_generation, round);
    db.reset_failed_action_for_retry(task_id, &test_key)?;
    let build_key = build_action_key(task_id, result_generation, round);
    db.reset_failed_action_for_retry(task_id, &build_key)?;
    let task = db.task(task_id)?;
    if task.state == TaskState::NeedsUserAction {
        db.transition_task(
            task_id,
            TaskState::Queued,
            Actor::User,
            Some("用户已提供所需信息，重新排队等待继续"),
            None,
            false,
            None,
        )
    } else {
        Ok(task)
    }
}

/// Codex re-review remediation (final Major closing Work Package 1): the
/// exact, structured marker [`run_build_for_round`] writes to a build
/// action's `error_summary` when — and only when — the round is genuinely
/// stalled on build detection being unable to determine whether a build
/// applies (`test_runner::BuildDetection::Insufficient` with no
/// confirmation yet found). This is a code-controlled constant, never
/// derived from user input, AI output, or a free-text status message meant
/// for a human reader — checking it by exact string equality below is
/// checking a stable internal signal, not "parsing free text" in the sense
/// that would be fragile or spoofable. A *different*, distinct marker is
/// written for every other way a build action can end up `Failed`
/// (allowlist rejection: `"build command not allowlisted"`; a genuine
/// build failure/timeout: `None`), so checking this exact string is
/// sufficient on its own to distinguish "detection insufficient" from
/// every other stop reason — see [`confirm_no_build_applicable`].
const BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER: &str =
    "build detection insufficient, awaiting user confirmation";

/// Maximum length (in bytes) Nova will store for a no-build confirmation's
/// free-text `reason` — a short user note, not a command-output dump (see
/// `cap_text`'s own 2000-byte cap used for real process stdout/stderr
/// elsewhere in this file).
const MAX_NO_BUILD_REASON_LEN: usize = 500;

/// Codex re-review remediation (fourth re-review, the final Major closing
/// Work Package 1): the full authorization/state/round/idempotency
/// boundary for recording a no-build confirmation. The backend is the
/// *sole* authority for which `result_generation`/`round` this
/// confirmation applies to and for whether the task is genuinely,
/// structurally stalled on build detection being unable to determine an
/// answer — nothing here trusts any caller-supplied generation, round,
/// task state, action key, or evidence status (the Tauri command wrapper
/// in `commands.rs` no longer even accepts a `round` parameter at all).
///
/// A thin public entry point — all of the actual coordination logic lives
/// in [`reconcile_no_build_confirmation`], named separately because it is
/// deliberately re-entrant: every call (the genuine first request *and*
/// every retry) runs the exact same function, which starts by asking "how
/// much of this operation has already happened?" rather than assuming a
/// found confirmation row means the whole operation is done.
pub fn confirm_no_build_applicable(
    db: &Database,
    project_id: &str,
    task_id: &str,
    reason: Option<&str>,
) -> NovaResult<crate::models::NoBuildConfirmation> {
    reconcile_no_build_confirmation(db, project_id, task_id, reason)
}

/// Codex re-review remediation (fourth re-review): recovers from the exact
/// half-success window the third re-review's implementation could get
/// stuck in — `record_no_build_confirmation` succeeding but the
/// reset-build-action-then-transition-task sequence that used to follow it
/// (previously factored into a small `unblock_build_execution_for_retry`
/// helper, now folded directly into this function so it can be re-driven
/// step by step) failing partway (a crash, a database error, the process
/// being killed) before completing. That implementation's idempotent
/// lookup treated
/// "a confirmation row exists" as synonymous with "the whole operation
/// finished", so a retry after a partial failure would hit the early
/// return and the task would stay stuck at `needs_user_action` forever
/// despite a confirmation already sitting in the database.
///
/// The fix: **a found confirmation is not evidence of completion** — it
/// only tells this function which `(result_generation, round)` to keep
/// coordinating for the *build action* and *task state*, which are
/// re-inspected fresh on every call. Every step below is naturally
/// idempotent and safe to re-run:
///
/// - Writing the confirmation row (`Database::record_no_build_confirmation`)
///   is itself `INSERT ... ON CONFLICT DO NOTHING` + re-select by natural
///   key — safe to attempt again even if it already happened.
/// - Resetting the build action (`reset_failed_action_for_retry`) only
///   touches a row whose status is still exactly `failed`; once reset, a
///   second attempt is a real no-op (its own `WHERE status = 'failed'`
///   guard matches zero rows).
/// - Transitioning the task only fires while it is still exactly
///   `needs_user_action`; once moved on, a second attempt correctly does
///   nothing.
///
/// This means a caller can retry this function an unbounded number of
/// times after a crash at *any* point in the sequence — the very next call
/// picks up from wherever the database actually is and drives forward only
/// the steps that are still outstanding, converging on the same terminal
/// state a single uninterrupted call would have reached. No SQLite
/// transaction spans the whole sequence; correctness comes from every
/// individual step being independently atomic (each is its own single
/// `UPDATE`/`INSERT` or `Database::transition_task`'s own already-atomic
/// state+event write) plus this function never trusting "a row exists" as
/// a substitute for re-checking the real, current state of everything else
/// before deciding what (if anything) is left to do.
///
/// Fails closed (returns `Err`, touches nothing) whenever the database
/// holds a combination that could not have arisen from this function's own
/// normal operation — e.g. a build action still `Failed` but carrying a
/// different marker than the one this confirmation is supposed to answer
/// for, or a task sitting in a state earlier than `needs_user_action` was
/// ever reachable from once a build stall (and therefore this
/// confirmation) could exist at all.
///
/// v0.1.0 scope correction (Work Package 2 remediation): a single call to
/// [`reconcile_no_build_confirmation_attempt`] is not, by itself, safe
/// under *true* concurrency — two overlapping calls can each read a
/// consistent snapshot of their own that the other invalidates mid-flight,
/// surfacing as a raw error even though the two calls are racing toward
/// the exact same outcome. Two distinct windows were found by repeated
/// stress testing:
///
/// 1. Both calls read [`Database::find_no_build_confirmation`] as `None`
///    before either has inserted (a genuine TOCTOU window across two
///    separate statements) — both proceed into the first-request gate,
///    but if one of them completes its *entire* attempt (insert, reset,
///    transition) before the other reaches its own re-validation of the
///    build action, the slower call sees an already-reset action and its
///    own gate rejects with "当前卡点不是构建检测无法判断" even though the
///    operation it was trying to perform has, in fact, already completed.
/// 2. Both calls resolve the *same* confirmation row (via the idempotent
///    insert) and both reach [`Database::transition_task`] with the task
///    still `needs_user_action`; one wins the CAS, the other's `WHERE
///    state = 'needs_user_action'` update matches zero rows and surfaces
///    "任务状态已被并发修改...已取消" even though the task is, at that
///    very moment, correctly `queued`.
///
/// Both windows share one property: by the time the losing call's error
/// surfaces, a confirmation row provably now exists for the *freshly
/// recomputed* `(result_generation, round)` — because nothing except a
/// call that itself passed the same first-request gate could have created
/// it. That is the one signal this wrapper trusts to distinguish "a
/// concurrent, legitimate call already converged this forward — retry and
/// pick up where it left off" from "this is a genuine authorization/data
/// failure that retrying can never fix" (wrong project, missing evidence,
/// a real marker mismatch, or one of Case F's structurally-impossible
/// early task states) — see [`no_build_confirmation_likely_converging`].
/// Retries are capped, re-run the exact same idempotent attempt (never a
/// different code path, never a raw SQL shortcut), and stop the moment
/// either a call succeeds or the convergence signal is absent.
fn reconcile_no_build_confirmation(
    db: &Database,
    project_id: &str,
    task_id: &str,
    reason: Option<&str>,
) -> NovaResult<crate::models::NoBuildConfirmation> {
    const MAX_CONCURRENT_CONVERGENCE_ATTEMPTS: u32 = 8;
    let mut last_err =
        NovaError::Config("无需构建确认协调未能在有限次重试内收敛，请人工检查".to_string());
    for attempt in 0..MAX_CONCURRENT_CONVERGENCE_ATTEMPTS {
        match reconcile_no_build_confirmation_attempt(db, project_id, task_id, reason) {
            Ok(confirmation) => return Ok(confirmation),
            Err(err) => {
                let is_last_attempt = attempt + 1 == MAX_CONCURRENT_CONVERGENCE_ATTEMPTS;
                last_err = err;
                if is_last_attempt {
                    break;
                }
                // Only retry when there is direct evidence — a
                // confirmation row that now exists where our own attempt
                // could not find or create one — that a concurrent call
                // already advanced this exact operation. Never retry a
                // failure that this same evidence check would also
                // observe on every subsequent attempt (an unrelated
                // project, missing evidence, a real marker mismatch, or a
                // structurally-impossible task state) — those fail closed
                // immediately, exactly as before this remediation.
                if !no_build_confirmation_likely_converging(db, project_id, task_id)? {
                    break;
                }
            }
        }
    }
    Err(last_err)
}

/// v0.1.0 scope correction (Work Package 2 remediation): the sole signal
/// [`reconcile_no_build_confirmation`] trusts before retrying a failed
/// attempt — re-reads the task fresh (never trusting anything read by the
/// failed attempt), recomputes `(result_generation, round)` fresh via
/// [`current_pending_round`] exactly like a brand new call would, and
/// checks whether a confirmation now exists for that exact natural key.
/// Deliberately excludes Case F's structurally-impossible early states
/// (`Draft`/`AwaitingConfirmation`/`Confirmed`) even if a confirmation
/// happens to exist, since no legitimate concurrent completion can ever
/// explain a task sitting in one of those states this late — retrying
/// would only ever reproduce the same fail-closed rejection.
fn no_build_confirmation_likely_converging(
    db: &Database,
    project_id: &str,
    task_id: &str,
) -> NovaResult<bool> {
    let task = match db.task(task_id) {
        Ok(task) => task,
        Err(_) => return Ok(false),
    };
    if task.project_id != project_id
        || matches!(
            task.state,
            TaskState::Draft | TaskState::AwaitingConfirmation | TaskState::Confirmed
        )
    {
        return Ok(false);
    }
    let result_generation = task.result_generation;
    let round = current_pending_round(db, project_id, task_id)?;
    Ok(db
        .find_no_build_confirmation(project_id, task_id, result_generation, round)?
        .is_some())
}

/// The single, non-retrying reconciliation attempt driven by
/// [`reconcile_no_build_confirmation`] — see that function's doc comment
/// for why a single attempt is not, by itself, safe under true
/// concurrency, and [`confirm_no_build_applicable`]'s doc comment for the
/// half-success recovery design this attempt itself implements.
fn reconcile_no_build_confirmation_attempt(
    db: &Database,
    project_id: &str,
    task_id: &str,
    reason: Option<&str>,
) -> NovaResult<crate::models::NoBuildConfirmation> {
    let task = db.task(task_id)?;
    if task.project_id != project_id {
        return Err(NovaError::Config(
            "task_id 与 project_id 不匹配，已拒绝".to_string(),
        ));
    }
    let result_generation = task.result_generation;
    let round = current_pending_round(db, project_id, task_id)?;
    let build_key = build_action_key(task_id, result_generation, round);

    let confirmation =
        match db.find_no_build_confirmation(project_id, task_id, result_generation, round)? {
            Some(existing) => existing,
            None => {
                // Case A (first genuine request): the full gate — never
                // skipped, never weakened, identical to the third re-review's
                // implementation — must hold before a confirmation is ever
                // written for the first time.
                if task.state != TaskState::NeedsUserAction {
                    return Err(NovaError::Config(format!(
                        "只有任务处于 needs_user_action 状态时才能确认无需构建，当前状态：{}",
                        task.state
                    )));
                }
                let action = db.find_action(task_id, &build_key)?.ok_or_else(|| {
                    NovaError::Config("当前不存在待确认的构建检测停滞，拒绝记录确认".to_string())
                })?;
                if action.status != ActionStatus::Failed
                    || action.error_summary.as_deref()
                        != Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER)
                {
                    return Err(NovaError::Config(
                        "当前卡点不是构建检测无法判断，拒绝记录无需构建确认".to_string(),
                    ));
                }
                let evidence_ok = db
                    .list_build_evidence_for_round(project_id, task_id, result_generation, round)?
                    .iter()
                    .any(|e| e.status == "insufficient_evidence");
                if !evidence_ok {
                    return Err(NovaError::Config(
                        "找不到匹配的构建证据不足记录，拒绝记录无需构建确认".to_string(),
                    ));
                }
                let effective_reason = match reason.map(str::trim) {
                    Some(text) if !text.is_empty() => cap_text(text, MAX_NO_BUILD_REASON_LEN),
                    _ => "用户确认该项目当前无需构建（未提供具体原因）".to_string(),
                };
                db.record_no_build_confirmation(
                    project_id,
                    task_id,
                    result_generation,
                    round,
                    Some(&effective_reason),
                )?
            }
        };

    // From here on, whether this call just wrote the confirmation (case A)
    // or found one already sitting there (cases B–F), the rest is the
    // exact same "finish whatever is left" coordination — re-inspecting
    // the real, current build action and task state rather than assuming
    // anything about how far a prior attempt got.
    if let Some(action) = db.find_action(task_id, &build_key)? {
        if action.status == ActionStatus::Failed {
            // Case B: the confirmation exists but the build action was
            // never reset — a prior attempt crashed between writing the
            // confirmation and resetting the action. Fail closed if the
            // action's own marker no longer matches what this
            // confirmation is supposed to be answering for (should not
            // happen in normal operation; only reachable via external
            // tampering or a genuine bug elsewhere) rather than blindly
            // resetting an action this confirmation was never actually
            // validated against.
            if action.error_summary.as_deref()
                != Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER)
            {
                return Err(NovaError::Config(
                    "构建动作当前状态与已记录的无需构建确认不一致，拒绝继续，请人工检查"
                        .to_string(),
                ));
            }
            db.reset_failed_action_for_retry(task_id, &build_key)?;
        }
        // Any other status (Planned/Started/Succeeded/Cancelled/
        // Uncertain) means the reset already happened (case C/D/E) or is
        // not this function's concern anymore — nothing further to do to
        // the action itself.
    }

    let task = db.task(task_id)?;
    match task.state {
        TaskState::NeedsUserAction => {
            // Case B (just reset above) or Case C: the action side is
            // settled: resume the task exactly the same way a single,
            // uninterrupted first call always has.
            db.transition_task(
                task_id,
                TaskState::Queued,
                Actor::User,
                Some("用户已确认该项目当前无需构建，重新排队等待继续"),
                None,
                false,
                None,
            )?;
        }
        TaskState::Draft | TaskState::AwaitingConfirmation | TaskState::Confirmed => {
            // Case F: a task that has ever reached a genuine build stall
            // (and therefore could ever have a real no-build confirmation
            // for it) can never legally be sitting in one of these three
            // *earlier-than-ever-started* states — no transition edge
            // leads back to any of them from `needs_user_action` or
            // anything reachable after it. A confirmation existing
            // alongside one of these states is a data inconsistency, not
            // a legitimate "already resumed further" case — fail closed
            // rather than silently pretending everything is fine.
            return Err(NovaError::Config(
                "任务当前状态与已存在的无需构建确认不一致（该状态不可能晚于确认发生），拒绝继续"
                    .to_string(),
            ));
        }
        _ => {
            // Cases D/E: Queued/Running/Verifying/SafeFixing/PreviewReady/
            // AwaitingUserAcceptance/Completed/Interrupted/Failed/
            // Cancelled — the task has already legitimately continued (or
            // terminated) past the point this confirmation unblocked.
            // Never roll it back, never reset anything, never repeat any
            // work — this is simply a repeated/late request for an
            // operation that already finished.
        }
    }

    Ok(confirmation)
}

/// v0.1.0 scope correction (Work Package 2, `CODEX_V0.1_SCOPE_CORRECTION_
/// AUDIT_09211027.md`): a read-only counterpart to [`current_pending_
/// round`] the frontend uses to decide whether to show the "confirm this
/// project currently needs no build" prompt for a `needs_user_action`
/// task — mirrors exactly the same real signals
/// [`reconcile_no_build_confirmation`]'s Case A gate requires before ever
/// accepting a confirmation (task ownership, `NeedsUserAction`, the
/// current generation/round's build action genuinely `Failed` with the
/// internal build-insufficient marker, and matching `insufficient_
/// evidence` build evidence), but never writes anything and never mutates
/// task/action/evidence state. Returns `Ok(false)` for every other
/// combination (wrong project, wrong task state, no matching action, a
/// genuine build failure/timeout/allowlist rejection, or a no-tests
/// stall instead) rather than erroring, since this is meant to be polled
/// freely by the UI alongside `detect_test_command` without the caller
/// having to pre-validate anything itself.
pub fn no_build_confirmation_needed(
    db: &Database,
    project_id: &str,
    task_id: &str,
) -> NovaResult<bool> {
    let task = db.task(task_id)?;
    if task.project_id != project_id || task.state != TaskState::NeedsUserAction {
        return Ok(false);
    }
    let result_generation = task.result_generation;
    let round = current_pending_round(db, project_id, task_id)?;
    let build_key = build_action_key(task_id, result_generation, round);
    let action = match db.find_action(task_id, &build_key)? {
        Some(action) => action,
        None => return Ok(false),
    };
    if action.status != ActionStatus::Failed
        || action.error_summary.as_deref() != Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER)
    {
        return Ok(false);
    }
    let evidence_ok = db
        .list_build_evidence_for_round(project_id, task_id, result_generation, round)?
        .iter()
        .any(|e| e.status == "insufficient_evidence");
    Ok(evidence_ok)
}

/// Outcome of [`run_dev_and_test_round`] — the developer-AI-plus-Nova's-
/// own-fact-verification portion shared by both the v0.1.0 single-AI path
/// ([`run_single_ai_round`]) and the dormant v0.2.0 dual-AI path
/// ([`run_one_round`], which continues on into a reviewer round only after
/// a [`DevTestOutcome::Verified`]).
enum DevTestOutcome {
    /// The developer AI ran, and Nova's own independent test/build
    /// verification for this round passed (or the round has no applicable
    /// tests, per a real user confirmation — see Blocker 3). Carries the
    /// redacted test summary text, used by the dormant v0.2.0 path to embed
    /// into the review package; the v0.1.0 path does not need it.
    Verified(String),
    /// Nova's own verification genuinely failed this round — never a
    /// reviewer's verdict. Carries the failure findings text, fed to the
    /// same developer AI as the one allowed automatic fix pass.
    NeedsFix(String),
    ActionUncertain,
    Stopped,
}

/// The developer-AI-plus-Nova's-own-verification portion of one round:
/// invoke the selected developer AI (skipped if this exact round's action
/// already succeeded, e.g. resuming after a crash), collect real git
/// evidence, then run Nova's own independent test/build verification
/// (Blocker 3 — never the developer AI's own claim). Shared unchanged by
/// both the v0.1.0 single-AI path and the dormant v0.2.0 dual-AI path, so a
/// fix here (e.g. to the test gate itself) can never silently apply to only
/// one of them.
#[allow(clippy::too_many_arguments)]
fn run_dev_and_test_round(
    db: &Database,
    task_id: &str,
    project_id: &str,
    project_dir: &Path,
    dev_kind: ToolKind,
    adapter: &dyn DevReviewAdapter,
    cancel: &CancellationToken,
    round: i64,
    prior_findings: Option<&str>,
    app_data_dir: &Path,
) -> NovaResult<DevTestOutcome> {
    let dev_key = format!("dev-{task_id}-round-{round}");
    match gate_action(db, task_id, "ai_development", &dev_key, false)? {
        ActionGate::Uncertain => {
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some("上一次开发 AI 执行结果不确定，需要用户确认后才能继续"),
                None,
                false,
                Some("action ledger uncertain: development"),
            )?;
            return Ok(DevTestOutcome::ActionUncertain);
        }
        ActionGate::AlreadySucceeded => {
            // A prior attempt already completed this exact round's
            // development step (crash-recovery resume after the dev run
            // committed but before the review step finished) — proceed
            // straight to evidence collection/review without re-invoking
            // the developer AI a second time for the same round.
        }
        ActionGate::Started => {
            let package_path =
                write_task_package(db, task_id, project_id, round, prior_findings, app_data_dir)?;
            let dev_request = DevelopmentRequest {
                ai_kind: dev_kind,
                project_dir: project_dir.to_path_buf(),
                package_path,
                timeout: DEV_TIMEOUT,
            };
            let result = adapter.run_development(&dev_request, cancel.clone());
            record_command_evidence(db, project_id, task_id, &result, "ai_development")?;
            let succeeded = matches!(result.outcome, ExecutionOutcome::Success);
            finish_gated_action(db, task_id, &dev_key, succeeded, &result)?;
            if !succeeded {
                // Major 4: a user cancellation must be recorded distinctly
                // from an ordinary process failure — never folded into the
                // same generic `failed` state a crash or non-zero exit
                // produces.
                if was_cancelled(&result) {
                    db.transition_task(
                        task_id,
                        TaskState::Cancelled,
                        Actor::User,
                        Some("用户在开发阶段取消了任务"),
                        None,
                        false,
                        None,
                    )?;
                } else {
                    db.transition_task(
                        task_id,
                        TaskState::Failed,
                        Actor::System,
                        Some("开发 AI 进程未能正常完成"),
                        None,
                        false,
                        None,
                    )?;
                }
                return Ok(DevTestOutcome::Stopped);
            }
        }
    }

    // Same reasoning as the review round's audit calls below: Nova's own
    // evidence-collection git commands must not be tied to the AI-process
    // cancellation token.
    collect_git_evidence(
        db,
        project_id,
        task_id,
        project_dir,
        &CancellationToken::new(),
    )?;

    // `verifying` covers both independent test execution and the review
    // step below — entered once, before either — so a test-only failure
    // (which returns early, never reaching the reviewer) still leaves the
    // task in a state `safe_fixing`/`failed` are legally reachable from
    // (`running -> safe_fixing` is deliberately *not* a legal edge in
    // Phase 2's accepted transition table; only `verifying -> safe_fixing`
    // is).
    db.transition_task(
        task_id,
        TaskState::Verifying,
        Actor::System,
        Some("独立测试与审核开始"),
        None,
        false,
        None,
    )?;

    // Blocker 3: independent test execution. Nova runs the test command
    // itself (never trusts the developer AI's own claim of "tests passed")
    // before the reviewer ever sees this round. A genuine test failure
    // means this round goes straight to `NeedsFix` — the reviewer is never
    // even asked, so its verdict can never "approve past" a real failure.
    let test_summary =
        match run_tests_for_round(db, project_id, task_id, project_dir, round, cancel)? {
            TestRoundOutcome::Passed(summary) => summary,
            TestRoundOutcome::NoTestsApplicable(summary) => summary,
            TestRoundOutcome::Failed(findings) => return Ok(DevTestOutcome::NeedsFix(findings)),
            TestRoundOutcome::AwaitingNoTestsConfirmation => {
                return Ok(DevTestOutcome::ActionUncertain)
            }
            TestRoundOutcome::ActionUncertain => return Ok(DevTestOutcome::ActionUncertain),
            TestRoundOutcome::Cancelled => return Ok(DevTestOutcome::Stopped),
        };

    // Codex re-review remediation (Major 3): tests passing is not enough —
    // "Nova 核验测试与构建" means Nova must also independently verify the
    // project actually builds before this round can be considered
    // verified. Never runs when the test step above already failed/stalled
    // (those branches already returned), so a build is only ever attempted
    // against a round whose tests are already known-good.
    match run_build_for_round(db, project_id, task_id, project_dir, round, cancel)? {
        BuildRoundOutcome::Passed(_) | BuildRoundOutcome::NoBuildApplicable(_) => {}
        BuildRoundOutcome::Failed(findings) => return Ok(DevTestOutcome::NeedsFix(findings)),
        BuildRoundOutcome::ActionUncertain => return Ok(DevTestOutcome::ActionUncertain),
        BuildRoundOutcome::Cancelled => return Ok(DevTestOutcome::Stopped),
    }

    Ok(DevTestOutcome::Verified(test_summary))
}

/// The reviewer-round continuation of one dormant v0.2.0 dual-AI round —
/// only ever reached via [`run_one_round`] after
/// [`run_dev_and_test_round`] returns [`DevTestOutcome::Verified`]. Kept as
/// a separate function purely so [`run_one_round`] itself stays a thin
/// "run dev+test, then if verified run review" dispatcher.
#[allow(clippy::too_many_arguments)]
fn run_review_round(
    db: &Database,
    task_id: &str,
    project_id: &str,
    project_dir: &Path,
    reviewer_kind: ToolKind,
    adapter: &dyn DevReviewAdapter,
    cancel: &CancellationToken,
    round: i64,
    app_data_dir: &Path,
    test_summary: &str,
) -> NovaResult<RoundOutcome> {
    let review_key = format!("review-{task_id}-round-{round}");
    match gate_action(db, task_id, "ai_review", &review_key, true)? {
        ActionGate::Uncertain => {
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some("上一次审核 AI 执行结果不确定，需要用户确认后才能继续"),
                None,
                false,
                Some("action ledger uncertain: review"),
            )?;
            return Ok(RoundOutcome::ActionUncertain);
        }
        ActionGate::AlreadySucceeded | ActionGate::Started => {}
    }

    // Nova's own act of writing the review package must never itself be
    // mistaken for a reviewer write — the before/after snapshot brackets
    // only the adapter invocation itself, not this preparation step.
    let package_path = write_review_package(
        db,
        task_id,
        project_id,
        round,
        app_data_dir,
        Some(test_summary),
    )?;

    // Blocker 1: the reviewer is never pointed at the real project. Nova
    // copies the working tree (minus `.git`, never following a symlink) into
    // a disposable, app-owned workspace first, and the review package is
    // copied alongside it — the reviewer's cwd/argv reference only this
    // copy, so it is physically unable to reach the original regardless of
    // what write access it has inside its own cwd. The git status/HEAD
    // before/after comparison below is retained too, but now purely as a
    // supplementary audit signal on the real project — the isolation itself
    // is the primary safety boundary.
    let workspace =
        match review_isolation::prepare(app_data_dir, project_id, task_id, round, project_dir) {
            Ok(workspace) => workspace,
            Err(error) => {
                let error_summary = redact_sensitive_text(&error.to_string());
                db.finish_action(
                    task_id,
                    &review_key,
                    ActionStatus::Failed,
                    None,
                    Some(&error_summary),
                )?;
                db.transition_task(
                    task_id,
                    TaskState::NeedsUserAction,
                    Actor::System,
                    Some("无法创建隔离审核工作区，审核未启动"),
                    None,
                    false,
                    Some("review workspace isolation setup failed"),
                )?;
                return Ok(RoundOutcome::ActionUncertain);
            }
        };
    let review_package_copy = workspace.root.join(
        package_path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("REVIEW_REQUEST.md")),
    );
    let _ = std::fs::copy(&package_path, &review_package_copy);

    // Nova's own before/after audit git calls deliberately use a fresh,
    // never-cancelled token, never the caller's `cancel` — that token
    // belongs to the *AI process*; if the user cancels while the reviewer
    // is running, this bookkeeping must still complete reliably rather than
    // itself being killed and spuriously reporting an "integrity violation"
    // (a `None` git result from a cancelled audit call would otherwise
    // compare unequal to the real `before` snapshot for no genuine reason).
    let audit_token = CancellationToken::new();
    let before_status = git_status_porcelain(project_dir, &audit_token)?.unwrap_or_default();
    let before_head = git_head(project_dir, &audit_token)?;

    let result_generation = db.task(task_id)?.result_generation;
    let review_request = ReviewRequest {
        ai_kind: reviewer_kind,
        isolated_workspace_dir: workspace.source_dir.clone(),
        package_path: review_package_copy,
        timeout: REVIEW_TIMEOUT,
        task_id: task_id.to_string(),
        result_generation,
        round,
    };
    let result = adapter.run_review(&review_request, cancel.clone());
    record_command_evidence(db, project_id, task_id, &result, "ai_review")?;

    let after_status = git_status_porcelain(project_dir, &audit_token)?.unwrap_or_default();
    let after_head = git_head(project_dir, &audit_token)?;
    let integrity_ok = before_status == after_status && before_head == after_head;

    // The isolated workspace is disposable and no longer needed once the
    // reviewer process has finished, regardless of verdict. Best-effort: if
    // cleanup itself fails, that is never a reason to fail the review round
    // (the workspace merely persists for later cleanup, harming nothing —
    // see `review_isolation::cleanup`'s own documentation).
    let _ = review_isolation::cleanup(app_data_dir, &workspace.root);

    let process_succeeded = matches!(result.outcome, ExecutionOutcome::Success);
    finish_gated_action(
        db,
        task_id,
        &review_key,
        process_succeeded && integrity_ok,
        &result,
    )?;

    if !integrity_ok {
        record_review(
            db,
            project_id,
            task_id,
            round,
            reviewer_kind,
            ReviewOrIntegrity::Integrity,
            None,
        )?;
        return Ok(RoundOutcome::IntegrityViolation);
    }

    if !process_succeeded {
        // Major 4: same cancelled/failed distinction as the developer step
        // above — a user cancellation during review must never be treated
        // as an ordinary "needs another fix pass" outcome.
        if was_cancelled(&result) {
            record_review(
                db,
                project_id,
                task_id,
                round,
                reviewer_kind,
                ReviewOrIntegrity::Verdict(ReviewVerdict::InsufficientEvidence),
                Some("用户在审核阶段取消了任务"),
            )?;
            db.transition_task(
                task_id,
                TaskState::Cancelled,
                Actor::User,
                Some("用户在审核阶段取消了任务"),
                None,
                false,
                None,
            )?;
            return Ok(RoundOutcome::Stopped);
        }
        record_review(
            db,
            project_id,
            task_id,
            round,
            reviewer_kind,
            ReviewOrIntegrity::Verdict(ReviewVerdict::InsufficientEvidence),
            Some("审核进程未能正常完成"),
        )?;
        return Ok(RoundOutcome::NeedsFix("审核进程未能正常完成".to_string()));
    }

    // Major 2: strict structured JSON verdict parsing — any parse failure
    // (missing/duplicate/extra fields, wrong task_id/generation/round,
    // multiple JSON candidates, truncated output, ...) uniformly routes to
    // `insufficient_evidence` rather than ever guessing a verdict or
    // silently defaulting to approve.
    let verdict_ctx = crate::runtime::verdict::VerdictContext {
        task_id,
        result_generation,
        round,
    };
    let (verdict, findings) =
        match crate::runtime::verdict::parse_structured_verdict(&result.stdout, &verdict_ctx) {
            Ok(parsed) => {
                let mut text = parsed.summary;
                if !parsed.findings.is_empty() {
                    text.push_str("\n\n");
                    text.push_str(&parsed.findings.join("\n"));
                }
                (parsed.verdict, redact_sensitive_text(&text))
            }
            Err(parse_error) => (
                ReviewVerdict::InsufficientEvidence,
                redact_sensitive_text(&format!("审核结论解析失败：{parse_error}")),
            ),
        };
    record_review(
        db,
        project_id,
        task_id,
        round,
        reviewer_kind,
        ReviewOrIntegrity::Verdict(verdict),
        Some(&findings),
    )?;

    match verdict {
        ReviewVerdict::Approve => Ok(RoundOutcome::Approved),
        ReviewVerdict::ChangesRequired | ReviewVerdict::InsufficientEvidence => {
            Ok(RoundOutcome::NeedsFix(findings))
        }
    }
}

/// Dormant v0.2.0 dual-AI round: developer AI + Nova verification, then (if
/// verified) a read-only reviewer round. Only reachable via
/// [`run_core_loop_dual_ai_v0_2`] — see that function's own doc comment for
/// why this is preserved but not wired into any command.
#[allow(clippy::too_many_arguments)]
fn run_one_round(
    db: &Database,
    task_id: &str,
    project_id: &str,
    project_dir: &Path,
    dev_kind: ToolKind,
    reviewer_kind: ToolKind,
    adapter: &dyn DevReviewAdapter,
    cancel: &CancellationToken,
    round: i64,
    prior_findings: Option<&str>,
    app_data_dir: &Path,
) -> NovaResult<RoundOutcome> {
    match run_dev_and_test_round(
        db,
        task_id,
        project_id,
        project_dir,
        dev_kind,
        adapter,
        cancel,
        round,
        prior_findings,
        app_data_dir,
    )? {
        DevTestOutcome::Verified(test_summary) => run_review_round(
            db,
            task_id,
            project_id,
            project_dir,
            reviewer_kind,
            adapter,
            cancel,
            round,
            app_data_dir,
            &test_summary,
        ),
        DevTestOutcome::NeedsFix(findings) => Ok(RoundOutcome::NeedsFix(findings)),
        DevTestOutcome::ActionUncertain => Ok(RoundOutcome::ActionUncertain),
        DevTestOutcome::Stopped => Ok(RoundOutcome::Stopped),
    }
}

/// v0.1.0 single-AI round outcome — deliberately has no `Approved` verdict
/// concept at all: `Verified` means exactly "the selected developer AI ran,
/// and Nova's own independent test/build verification passed (or has a
/// real no-tests confirmation)" — never a second AI's opinion.
enum SingleAiRoundOutcome {
    Verified,
    NeedsFix(String),
    ActionUncertain,
    Stopped,
}

/// v0.1.0 scope correction: one development-AI-plus-Nova-verification
/// round with **no reviewer step at all** — the actual v0.1.0 product main
/// path (`导入项目 -> ... -> 一个开发 AI 修改代码 -> Nova 核验文件和 Git 变化
/// -> Nova 核验测试与构建 -> ... -> 效果预览 -> 用户验收`). Reuses
/// [`run_dev_and_test_round`] unchanged — the same developer-AI invocation,
/// Action Ledger gating, git evidence collection, and independent test/
/// build verification the dormant v0.2.0 dual-AI path uses — so this is
/// never a weaker or differently-verified path, only a shorter one.
#[allow(clippy::too_many_arguments)]
fn run_single_ai_round(
    db: &Database,
    task_id: &str,
    project_id: &str,
    project_dir: &Path,
    dev_kind: ToolKind,
    adapter: &dyn DevReviewAdapter,
    cancel: &CancellationToken,
    round: i64,
    prior_findings: Option<&str>,
    app_data_dir: &Path,
) -> NovaResult<SingleAiRoundOutcome> {
    match run_dev_and_test_round(
        db,
        task_id,
        project_id,
        project_dir,
        dev_kind,
        adapter,
        cancel,
        round,
        prior_findings,
        app_data_dir,
    )? {
        DevTestOutcome::Verified(_test_summary) => Ok(SingleAiRoundOutcome::Verified),
        DevTestOutcome::NeedsFix(findings) => Ok(SingleAiRoundOutcome::NeedsFix(findings)),
        DevTestOutcome::ActionUncertain => Ok(SingleAiRoundOutcome::ActionUncertain),
        DevTestOutcome::Stopped => Ok(SingleAiRoundOutcome::Stopped),
    }
}

/// Outcome of [`run_tests_for_round`], driving `run_one_round`'s next step.
#[derive(Debug)]
enum TestRoundOutcome {
    /// Tests ran and passed (or Nova could not determine an applicable test
    /// command at all — see the `NoTestsApplicable` case below for that
    /// distinct situation). Carries the redacted summary text embedded into
    /// the review package.
    Passed(String),
    /// No recognizable test command exists for this project — an explicit,
    /// Nova-determined `no_tests_applicable` fact (never an AI's claim),
    /// carried forward so this is never confused with "tests ran and
    /// passed".
    NoTestsApplicable(String),
    /// Tests ran and genuinely failed — the round goes straight to
    /// `NeedsFix` without ever invoking the reviewer.
    Failed(String),
    /// Second re-review (Blocker 3): no test command was detected or
    /// configured, *and* no real `no_tests_confirmations` row exists for
    /// this exact `(task_id, result_generation, round)` — Nova refuses to
    /// record `no_tests_applicable` on its own say-so and stops for the
    /// user to either confirm there genuinely are no applicable tests or
    /// provide a test command instead.
    AwaitingNoTestsConfirmation,
    ActionUncertain,
    Cancelled,
}

/// Work Package 3 remediation (Minor 3 carried from the build-cancellation
/// stability re-review): the test Action Ledger key must be bound to
/// `result_generation`, not just `task_id`/`round` — mirrors
/// [`build_action_key`] exactly, for the same reason. Before this fix, a
/// task that completed round 1 (bumping `result_generation`), was rejected,
/// and was requeued for a fresh round 1 at the *new* generation would find
/// `gate_action` reporting `AlreadySucceeded` against the *old* generation's
/// identically-keyed row — the old generation's test never actually ran
/// again, and [`run_tests_for_round`]'s evidence lookup (already
/// generation/round-scoped) would then find nothing for the new generation
/// and silently mis-report `NoTestsApplicable`.
fn test_action_key(task_id: &str, result_generation: i64, round: i64) -> String {
    format!("test-{task_id}-generation-{result_generation}-round-{round}")
}

/// Blocker 3: determines a safe test command (the task's own confirmed
/// command if set, otherwise a fresh auto-detection attempt — always
/// re-validated against [`test_runner::is_allowlisted`] before ever
/// executing anything), runs it through Phase 1's safe process boundary,
/// and records the outcome as independent `test_evidence` — never trusting
/// anything the developer AI claimed. Gated through the Action Ledger with
/// its own generation-aware [`test_action_key`], exactly like the build
/// step, so a crash mid-run is never silently re-run, each round always
/// gets its own fresh key, and a stale `succeeded` action from an earlier
/// generation can never be mistaken for proof the *current* generation's
/// test already ran.
fn run_tests_for_round(
    db: &Database,
    project_id: &str,
    task_id: &str,
    project_dir: &Path,
    round: i64,
    cancel: &CancellationToken,
) -> NovaResult<TestRoundOutcome> {
    let result_generation = db.task(task_id)?.result_generation;
    let test_key = test_action_key(task_id, result_generation, round);
    match gate_action(db, task_id, "test_execution", &test_key, false)? {
        ActionGate::Uncertain => {
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some("上一次独立测试执行结果不确定，需要用户确认后才能继续"),
                None,
                false,
                Some("action ledger uncertain: test_execution"),
            )?;
            return Ok(TestRoundOutcome::ActionUncertain);
        }
        ActionGate::AlreadySucceeded => {
            // A prior attempt already recorded this exact round's test
            // evidence (crash-recovery resume) — reuse it rather than
            // re-running, but only ever for *this* round's own key, never
            // an earlier round's (Blocker 3's "第二轮必须重新运行测试" is
            // satisfied structurally: round 2 always uses a different key).
            //
            // Work Package 3 remediation: an `AlreadySucceeded` action alone
            // is not sufficient proof — it must be corroborated by a real
            // `test_evidence` row for this *exact* (result_generation,
            // round). Since the key itself is now generation-aware, a
            // mismatch here would mean the Action Ledger and evidence
            // tables have drifted out of sync (should not happen under
            // normal operation) — fail closed rather than ever reporting
            // `NoTestsApplicable` on missing evidence (mirrors
            // `run_build_for_round`'s identical fix).
            let existing =
                db.list_test_evidence_for_round(project_id, task_id, result_generation, round)?;
            if let Some(evidence) = existing.into_iter().next_back() {
                return Ok(test_round_outcome_from_status(&evidence));
            }
            db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
                project_id,
                task_id,
                result_generation,
                round,
                test_command_redacted: None,
                status: "insufficient_evidence",
                passed_count: 0,
                failed_count: 0,
                skipped_count: 0,
                exit_code: None,
                reason: Some(
                    "测试动作已记录为成功，但找不到对应 generation/round 的测试证据，已按不确定处理",
                ),
                stdout_ref: None,
                stderr_ref: None,
            })?;
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some("测试证据缺失，无法确认本轮测试结果，需要用户处理"),
                None,
                false,
                Some("test action succeeded but no matching test_evidence found"),
            )?;
            return Ok(TestRoundOutcome::ActionUncertain);
        }
        ActionGate::Started => {}
    }

    let task = db.task(task_id)?;

    let command = match (&task.test_command_redacted, &task.test_command_source) {
        (Some(command_text), Some(_source)) => parse_stored_command(command_text),
        _ => test_runner::detect_test_command(project_dir),
    };

    let Some(command) = command else {
        // Second re-review (Blocker 3): a missing test command may only
        // become `no_tests_applicable` evidence once a real, persisted user
        // confirmation exists for *this exact* generation/round — never on
        // Nova's own detection failure alone, and never reused from a prior
        // round or a prior result generation (schema `UNIQUE(task_id,
        // result_generation, round)` plus this exact-match lookup enforce
        // that no wider reuse is even possible).
        let confirmation =
            db.find_no_tests_confirmation(project_id, task_id, result_generation, round)?;
        let Some(confirmation) = confirmation else {
            db.finish_action(
                task_id,
                &test_key,
                ActionStatus::Failed,
                None,
                Some("no_tests_applicable requires an explicit user confirmation, none found for this generation/round"),
            )?;
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some(
                    "未检测到可运行的自动测试，需要用户确认该项目当前没有适用测试，或提供测试命令",
                ),
                None,
                false,
                Some("no_tests_applicable awaiting explicit user confirmation"),
            )?;
            return Ok(TestRoundOutcome::AwaitingNoTestsConfirmation);
        };
        let reason = format!(
            "用户已确认该项目当前没有适用测试（确认记录 {}，确认时间 {}）",
            confirmation.id, confirmation.confirmed_at
        );
        let evidence =
            db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
                project_id,
                task_id,
                result_generation,
                round,
                test_command_redacted: None,
                status: "no_tests_applicable",
                passed_count: 0,
                failed_count: 0,
                skipped_count: 0,
                exit_code: None,
                reason: Some(&reason),
                stdout_ref: None,
                stderr_ref: None,
            })?;
        db.finish_action(task_id, &test_key, ActionStatus::Succeeded, None, None)?;
        return Ok(TestRoundOutcome::NoTestsApplicable(
            evidence.reason.unwrap_or_default(),
        ));
    };

    if !test_runner::is_allowlisted(&command.program, &command.args) {
        db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
            project_id,
            task_id,
            result_generation,
            round,
            test_command_redacted: Some(&command.display()),
            status: "insufficient_evidence",
            passed_count: 0,
            failed_count: 0,
            skipped_count: 0,
            exit_code: None,
            reason: Some("测试命令未通过安全允许列表校验，已拒绝执行"),
            stdout_ref: None,
            stderr_ref: None,
        })?;
        db.finish_action(
            task_id,
            &test_key,
            ActionStatus::Failed,
            None,
            Some("test command not allowlisted"),
        )?;
        db.transition_task(
            task_id,
            TaskState::NeedsUserAction,
            Actor::System,
            Some("配置的测试命令未通过安全校验，需要用户确认"),
            None,
            false,
            Some("test command failed allowlist check"),
        )?;
        return Ok(TestRoundOutcome::ActionUncertain);
    }

    let outcome = test_runner::run_tests(
        &command.program,
        &command.args,
        project_dir,
        TEST_TIMEOUT,
        cancel.clone(),
    );

    let command_display = command.display();
    if outcome.cancelled {
        db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
            project_id,
            task_id,
            result_generation,
            round,
            test_command_redacted: Some(&command_display),
            status: "cancelled",
            passed_count: 0,
            failed_count: 0,
            skipped_count: 0,
            exit_code: outcome.exit_code.map(i64::from),
            reason: Some("用户在测试阶段取消了任务"),
            stdout_ref: None,
            stderr_ref: None,
        })?;
        db.finish_action(
            task_id,
            &test_key,
            ActionStatus::Cancelled,
            None,
            Some("user cancellation during test execution"),
        )?;
        db.transition_task(
            task_id,
            TaskState::Cancelled,
            Actor::User,
            Some("用户在测试阶段取消了任务"),
            None,
            false,
            None,
        )?;
        return Ok(TestRoundOutcome::Cancelled);
    }

    let status = if outcome.timed_out {
        "timed_out"
    } else if !outcome.process_succeeded || outcome.failed.unwrap_or(0) > 0 {
        "failed"
    } else {
        "passed"
    };
    let stdout_ref = redact_sensitive_text(&cap_text(&outcome.stdout, 2000));
    let stderr_ref = redact_sensitive_text(&cap_text(&outcome.stderr, 2000));
    db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
        project_id,
        task_id,
        result_generation,
        round,
        test_command_redacted: Some(&command_display),
        status,
        passed_count: outcome.passed.unwrap_or(0),
        failed_count: outcome.failed.unwrap_or(0),
        skipped_count: outcome.skipped.unwrap_or(0),
        exit_code: outcome.exit_code.map(i64::from),
        reason: if outcome.timed_out {
            Some("测试执行超时")
        } else {
            None
        },
        stdout_ref: Some(&stdout_ref),
        stderr_ref: Some(&stderr_ref),
    })?;
    db.finish_action(
        task_id,
        &test_key,
        if status == "passed" {
            ActionStatus::Succeeded
        } else {
            ActionStatus::Failed
        },
        None,
        None,
    )?;

    let summary = format!(
        "命令：{command_display}\n实际程序：{}\nargv：{:?}\n工作目录：{}\n结果：{status}\n通过：{}，失败：{}，跳过：{}\n退出码：{}",
        outcome.program,
        outcome.args,
        outcome.cwd,
        outcome.passed.map(|v| v.to_string()).unwrap_or_else(|| "未知".to_string()),
        outcome.failed.map(|v| v.to_string()).unwrap_or_else(|| "未知".to_string()),
        outcome.skipped.map(|v| v.to_string()).unwrap_or_else(|| "未知".to_string()),
        outcome.exit_code.map(|v| v.to_string()).unwrap_or_else(|| "无".to_string()),
    );
    if status == "passed" {
        Ok(TestRoundOutcome::Passed(redact_sensitive_text(&summary)))
    } else {
        Ok(TestRoundOutcome::Failed(redact_sensitive_text(&format!(
            "独立测试执行未通过，需要在下一轮修复：\n{summary}"
        ))))
    }
}

fn test_round_outcome_from_status(evidence: &crate::models::TestEvidence) -> TestRoundOutcome {
    match evidence.status.as_str() {
        "passed" => TestRoundOutcome::Passed(format!(
            "（复用本轮已记录的测试证据）通过：{}，失败：{}",
            evidence.passed_count, evidence.failed_count
        )),
        "no_tests_applicable" => {
            TestRoundOutcome::NoTestsApplicable(evidence.reason.clone().unwrap_or_default())
        }
        _ => TestRoundOutcome::Failed(format!(
            "（复用本轮已记录的测试证据）状态：{}",
            evidence.status
        )),
    }
}

/// Outcome of [`run_build_for_round`] — mirrors [`TestRoundOutcome`]'s
/// shape exactly. The `Passed`/`NoBuildApplicable` payload strings are
/// carried for symmetry with `TestRoundOutcome` (and potential future use,
/// e.g. embedding into a review package the way `DevTestOutcome::Verified`
/// does) but are not currently read by `run_dev_and_test_round`'s match
/// arms, hence `#[allow(dead_code)]`.
#[allow(dead_code)]
enum BuildRoundOutcome {
    Passed(String),
    NoBuildApplicable(String),
    Failed(String),
    ActionUncertain,
    Cancelled,
}

/// Codex re-review remediation (Major 3, second re-review): the build
/// Action Ledger key must be bound to `result_generation`, not just
/// `task_id`/`round` — otherwise a stale `succeeded` action from an
/// earlier generation could be mistaken for proof the *current*
/// generation's build already ran.
fn build_action_key(task_id: &str, result_generation: i64, round: i64) -> String {
    format!("build-{task_id}-generation-{result_generation}-round-{round}")
}

/// Codex re-review remediation (Major 3, first re-review): Nova's own
/// independent build verification, run immediately after a round's
/// independent tests pass (or are not applicable) — mirrors
/// [`run_tests_for_round`]'s structure and safety discipline exactly
/// (Action Ledger gate, real allowlist check before executing anything,
/// real `build_evidence` recorded independent of anything the developer AI
/// claimed).
///
/// Second re-review remediation (Major 2 + Major 3): the Action Ledger key
/// is now generation-aware (see [`build_action_key`]), and a genuine
/// `AlreadySucceeded` gate result is no longer trusted on its own — it must
/// be corroborated by a real `build_evidence` row for the *exact same*
/// `(result_generation, round)`, or this fails closed rather than silently
/// reporting `NoBuildApplicable`. Detection failure (`BuildDetection::
/// Insufficient`) is handled distinctly from a positively-confirmed
/// `NotApplicable`: the former requires a real, persisted user confirmation
/// (mirroring Blocker 3's `no_tests_applicable` confirmation mechanism)
/// before it may be treated as "no build needed"; the latter is a
/// mechanically-verified fact from `Cargo.toml`/`package.json` itself and
/// needs no confirmation.
fn run_build_for_round(
    db: &Database,
    project_id: &str,
    task_id: &str,
    project_dir: &Path,
    round: i64,
    cancel: &CancellationToken,
) -> NovaResult<BuildRoundOutcome> {
    let result_generation = db.task(task_id)?.result_generation;
    let build_key = build_action_key(task_id, result_generation, round);
    match gate_action(db, task_id, "build_execution", &build_key, false)? {
        ActionGate::Uncertain => {
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some("上一次独立构建执行结果不确定，需要用户确认后才能继续"),
                None,
                false,
                Some("action ledger uncertain: build_execution"),
            )?;
            return Ok(BuildRoundOutcome::ActionUncertain);
        }
        ActionGate::AlreadySucceeded => {
            // Codex re-review remediation (Major 3): an `AlreadySucceeded`
            // action alone is not sufficient proof — it must be
            // corroborated by a real build_evidence row for this *exact*
            // (result_generation, round). Since the key itself is now
            // generation-aware, a mismatch here would mean the Action
            // Ledger and evidence tables have drifted out of sync (should
            // not happen under normal operation) — fail closed rather than
            // ever reporting `NoBuildApplicable` on missing evidence.
            let existing =
                db.list_build_evidence_for_round(project_id, task_id, result_generation, round)?;
            if let Some(evidence) = existing.into_iter().next_back() {
                return Ok(build_round_outcome_from_status(&evidence));
            }
            db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
                project_id,
                task_id,
                result_generation,
                round,
                build_command_redacted: None,
                status: "insufficient_evidence",
                exit_code: None,
                reason: Some(
                    "构建动作已记录为成功，但找不到对应 generation/round 的构建证据，已按不确定处理",
                ),
                stdout_ref: None,
                stderr_ref: None,
            })?;
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some("构建证据缺失，无法确认本轮构建结果，需要用户处理"),
                None,
                false,
                Some("build action succeeded but no matching build_evidence found"),
            )?;
            return Ok(BuildRoundOutcome::ActionUncertain);
        }
        ActionGate::Started => {}
    }

    let command = match test_runner::detect_build_command(project_dir) {
        test_runner::BuildDetection::NotApplicable(reason) => {
            let evidence =
                db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
                    project_id,
                    task_id,
                    result_generation,
                    round,
                    build_command_redacted: None,
                    status: "no_build_applicable",
                    exit_code: None,
                    reason: Some(&reason),
                    stdout_ref: None,
                    stderr_ref: None,
                })?;
            db.finish_action(task_id, &build_key, ActionStatus::Succeeded, None, None)?;
            return Ok(BuildRoundOutcome::NoBuildApplicable(
                evidence.reason.unwrap_or_default(),
            ));
        }
        test_runner::BuildDetection::Insufficient(reason) => {
            // Codex re-review remediation (Major 2): detection failure must
            // never be treated as "no build needed" on its own — only a
            // real, persisted user confirmation (mirroring Blocker 3's
            // no-tests-applicable mechanism) may unblock this.
            if let Some(confirmation) =
                db.find_no_build_confirmation(project_id, task_id, result_generation, round)?
            {
                let confirmed_reason = format!(
                    "用户已确认该项目当前无需构建（确认记录 {}，确认时间 {}）",
                    confirmation.id, confirmation.confirmed_at
                );
                let evidence = db.record_build_evidence(
                    &crate::storage::evidence::RecordBuildEvidenceRequest {
                        project_id,
                        task_id,
                        result_generation,
                        round,
                        build_command_redacted: None,
                        status: "no_build_applicable",
                        exit_code: None,
                        reason: Some(&confirmed_reason),
                        stdout_ref: None,
                        stderr_ref: None,
                    },
                )?;
                db.finish_action(task_id, &build_key, ActionStatus::Succeeded, None, None)?;
                return Ok(BuildRoundOutcome::NoBuildApplicable(
                    evidence.reason.unwrap_or_default(),
                ));
            }
            db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
                project_id,
                task_id,
                result_generation,
                round,
                build_command_redacted: None,
                status: "insufficient_evidence",
                exit_code: None,
                reason: Some(&reason),
                stdout_ref: None,
                stderr_ref: None,
            })?;
            db.finish_action(
                task_id,
                &build_key,
                ActionStatus::Failed,
                None,
                Some("build detection insufficient, awaiting user confirmation"),
            )?;
            db.transition_task(
                task_id,
                TaskState::NeedsUserAction,
                Actor::System,
                Some(
                    "无法可靠判断该项目是否需要构建，需要用户确认该项目当前无需构建，或提供受支持的构建方式",
                ),
                None,
                false,
                Some("no_build_applicable requires an explicit user confirmation, none found for this generation/round"),
            )?;
            return Ok(BuildRoundOutcome::ActionUncertain);
        }
        test_runner::BuildDetection::Detected(command) => command,
    };

    if !test_runner::is_build_allowlisted(&command.program, &command.args) {
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id,
            task_id,
            result_generation,
            round,
            build_command_redacted: Some(&command.display()),
            status: "insufficient_evidence",
            exit_code: None,
            reason: Some("构建命令未通过安全允许列表校验，已拒绝执行"),
            stdout_ref: None,
            stderr_ref: None,
        })?;
        db.finish_action(
            task_id,
            &build_key,
            ActionStatus::Failed,
            None,
            Some("build command not allowlisted"),
        )?;
        db.transition_task(
            task_id,
            TaskState::NeedsUserAction,
            Actor::System,
            Some("检测到的构建命令未通过安全校验，需要用户确认"),
            None,
            false,
            Some("build command failed allowlist check"),
        )?;
        return Ok(BuildRoundOutcome::ActionUncertain);
    }

    let outcome = test_runner::run_tests(
        &command.program,
        &command.args,
        project_dir,
        TEST_TIMEOUT,
        cancel.clone(),
    );

    let command_display = command.display();
    if outcome.cancelled {
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id,
            task_id,
            result_generation,
            round,
            build_command_redacted: Some(&command_display),
            status: "cancelled",
            exit_code: outcome.exit_code.map(i64::from),
            reason: Some("用户在构建阶段取消了任务"),
            stdout_ref: None,
            stderr_ref: None,
        })?;
        db.finish_action(
            task_id,
            &build_key,
            ActionStatus::Cancelled,
            None,
            Some("user cancellation during build execution"),
        )?;
        db.transition_task(
            task_id,
            TaskState::Cancelled,
            Actor::User,
            Some("用户在构建阶段取消了任务"),
            None,
            false,
            None,
        )?;
        return Ok(BuildRoundOutcome::Cancelled);
    }

    let status = if outcome.timed_out {
        "timed_out"
    } else if !outcome.process_succeeded {
        "failed"
    } else {
        "passed"
    };
    let stdout_ref = redact_sensitive_text(&cap_text(&outcome.stdout, 2000));
    let stderr_ref = redact_sensitive_text(&cap_text(&outcome.stderr, 2000));
    db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
        project_id,
        task_id,
        result_generation,
        round,
        build_command_redacted: Some(&command_display),
        status,
        exit_code: outcome.exit_code.map(i64::from),
        reason: if outcome.timed_out {
            Some("构建执行超时")
        } else {
            None
        },
        stdout_ref: Some(&stdout_ref),
        stderr_ref: Some(&stderr_ref),
    })?;
    db.finish_action(
        task_id,
        &build_key,
        if status == "passed" {
            ActionStatus::Succeeded
        } else {
            ActionStatus::Failed
        },
        None,
        None,
    )?;

    let summary = format!(
        "命令：{command_display}\n实际程序：{}\nargv：{:?}\n工作目录：{}\n结果：{status}\n退出码：{}",
        outcome.program,
        outcome.args,
        outcome.cwd,
        outcome
            .exit_code
            .map(|v| v.to_string())
            .unwrap_or_else(|| "无".to_string()),
    );
    if status == "passed" {
        Ok(BuildRoundOutcome::Passed(redact_sensitive_text(&summary)))
    } else {
        Ok(BuildRoundOutcome::Failed(redact_sensitive_text(&format!(
            "独立构建执行未通过，需要在下一轮修复：\n{summary}"
        ))))
    }
}

fn build_round_outcome_from_status(evidence: &crate::models::BuildEvidence) -> BuildRoundOutcome {
    match evidence.status.as_str() {
        "passed" => BuildRoundOutcome::Passed("（复用本轮已记录的构建证据）通过".to_string()),
        "no_build_applicable" => {
            BuildRoundOutcome::NoBuildApplicable(evidence.reason.clone().unwrap_or_default())
        }
        _ => BuildRoundOutcome::Failed(format!(
            "（复用本轮已记录的构建证据）状态：{}",
            evidence.status
        )),
    }
}

/// A task's stored `test_command_redacted` is saved as a single
/// space-separated display string (see [`test_runner::DetectedCommand::display`]);
/// this reverses that for execution. Deliberately naive (splits on
/// whitespace) — matches the only shapes [`test_runner::is_allowlisted`]
/// ever accepts, none of which need quoting.
fn parse_stored_command(text: &str) -> Option<test_runner::DetectedCommand> {
    let mut parts = text.split_whitespace();
    let program = parts.next()?.to_string();
    let args = parts.map(str::to_string).collect();
    Some(test_runner::DetectedCommand { program, args })
}

fn cap_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    // Byte-length cap, but truncated only on a real UTF-8 char boundary —
    // this text may contain multi-byte (e.g. Chinese) characters, and a raw
    // byte-index slice could otherwise panic mid-character.
    let mut end = max_len;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... (截断)", &text[..end])
}

enum ActionGate {
    /// The action was already `planned` and has just been marked
    /// `started` by this call — the caller must actually perform the work.
    Started,
    /// The action already reached `succeeded` in an earlier attempt — the
    /// caller must not repeat the underlying side effect.
    AlreadySucceeded,
    /// The action is `uncertain` (crash-recovery) and cannot be
    /// automatically retried — the caller must stop for the user.
    Uncertain,
}

fn gate_action(
    db: &Database,
    task_id: &str,
    action_type: &str,
    key: &str,
    is_read_only: bool,
) -> NovaResult<ActionGate> {
    let planned = db.plan_action(task_id, action_type, key)?;
    if planned.status == ActionStatus::Uncertain {
        return Ok(ActionGate::Uncertain);
    }
    if planned.status == ActionStatus::Succeeded {
        return Ok(ActionGate::AlreadySucceeded);
    }
    match db.start_action(task_id, key, is_read_only) {
        Ok(_) => Ok(ActionGate::Started),
        Err(_) => {
            // Re-check: a concurrent/earlier run may have already finished
            // this action between the plan_action read above and this
            // start_action attempt.
            match db.find_action(task_id, key)? {
                Some(action) if action.status == ActionStatus::Succeeded => {
                    Ok(ActionGate::AlreadySucceeded)
                }
                Some(action) if action.status == ActionStatus::Uncertain => {
                    Ok(ActionGate::Uncertain)
                }
                _ => Ok(ActionGate::Uncertain),
            }
        }
    }
}

/// Major 4: distinguishes a user cancellation from any other process
/// failure (crash, non-zero exit, timeout, start failure, ...) so the two
/// are never folded into the same `failed` task state.
fn was_cancelled(result: &ExecutionResult) -> bool {
    matches!(
        result.outcome,
        ExecutionOutcome::Failed {
            kind: ExecutionErrorKind::Cancelled,
            ..
        }
    )
}

fn finish_gated_action(
    db: &Database,
    task_id: &str,
    key: &str,
    succeeded: bool,
    result: &crate::runtime::types::ExecutionResult,
) -> NovaResult<()> {
    // Re-review Major 4: a user cancellation must land on the Action
    // Ledger's own `cancelled` terminal status, never `failed` — folding
    // the two together made a cancelled action indistinguishable from a
    // genuine process failure in every downstream reader of evidence
    // (recovery sweeps, the seven documents, a human auditing the ledger).
    let status = if succeeded {
        ActionStatus::Succeeded
    } else if was_cancelled(result) {
        ActionStatus::Cancelled
    } else {
        ActionStatus::Failed
    };
    let error_summary = match &result.outcome {
        ExecutionOutcome::Failed {
            detail,
            kind: ExecutionErrorKind::Cancelled,
        } => Some(format!(
            "user cancellation: {}",
            redact_sensitive_text(detail)
        )),
        ExecutionOutcome::Failed { detail, .. } => Some(redact_sensitive_text(detail)),
        ExecutionOutcome::Success => None,
    };
    db.finish_action(task_id, key, status, None, error_summary.as_deref())?;
    Ok(())
}

/// Minor fix: `stdout_ref`/`stderr_ref` are now real, redacted, length-capped
/// summaries of what the process actually produced — never hardcoded
/// `None`. `result.stdout`/`result.stderr` have already been through Phase
/// 1's own redaction once; `redact_sensitive_text` here is defense in depth
/// (matches every other evidence-writing function in this codebase), and
/// `cap_text` bounds storage size independent of Phase 1's own output cap.
fn record_command_evidence(
    db: &Database,
    project_id: &str,
    task_id: &str,
    result: &crate::runtime::types::ExecutionResult,
    label: &str,
) -> NovaResult<()> {
    let command_text = format!("{label}: {} {}", result.program, result.args.join(" "));
    let stdout_ref = redact_sensitive_text(&cap_text(&result.stdout, 2000));
    let stderr_ref = redact_sensitive_text(&cap_text(&result.stderr, 2000));
    db.record_command_evidence(
        project_id,
        task_id,
        &command_text,
        result.exit_code.map(i64::from),
        Some(&result.started_at.to_rfc3339()),
        Some(&result.ended_at.to_rfc3339()),
        if stdout_ref.is_empty() {
            None
        } else {
            Some(stdout_ref.as_str())
        },
        if stderr_ref.is_empty() {
            None
        } else {
            Some(stderr_ref.as_str())
        },
    )?;
    Ok(())
}

enum ReviewOrIntegrity {
    Verdict(ReviewVerdict),
    Integrity,
}

fn record_review(
    db: &Database,
    project_id: &str,
    task_id: &str,
    round: i64,
    reviewer_kind: ToolKind,
    outcome: ReviewOrIntegrity,
    findings: Option<&str>,
) -> NovaResult<()> {
    let task = db.task(task_id)?;
    let verdict_str = match outcome {
        ReviewOrIntegrity::Verdict(v) => v.as_str(),
        ReviewOrIntegrity::Integrity => "integrity_violation",
    };
    db.record_task_review(
        project_id,
        task_id,
        task.result_generation,
        round,
        ai_adapter::ai_kind_to_str(reviewer_kind),
        verdict_str,
        findings,
    )?;
    Ok(())
}

fn collect_git_evidence(
    db: &Database,
    project_id: &str,
    task_id: &str,
    project_dir: &Path,
    cancel: &CancellationToken,
) -> NovaResult<()> {
    let branch = git_output(project_dir, &["rev-parse", "--abbrev-ref", "HEAD"], cancel)?;
    let diff_stat = git_output(project_dir, &["diff", "--stat"], cancel)?;
    db.record_git_evidence(
        project_id,
        task_id,
        "post_development_diff",
        None,
        branch.as_deref(),
        diff_stat.as_deref(),
    )?;

    if let Some(status) = git_status_porcelain(project_dir, cancel)? {
        for line in status.lines() {
            if line.len() < 4 {
                continue;
            }
            let code = &line[0..2];
            let path = line[3..].trim();
            let change_kind = if code.contains('D') {
                "deleted"
            } else if code.contains('?') || code.contains('A') {
                "created"
            } else {
                "modified"
            };
            let _ = db.record_file_change_evidence(project_id, task_id, path, change_kind, None);
        }
    }
    Ok(())
}

fn git_status_porcelain(
    project_dir: &Path,
    cancel: &CancellationToken,
) -> NovaResult<Option<String>> {
    git_output(project_dir, &["status", "--porcelain"], cancel)
}

fn git_head(project_dir: &Path, cancel: &CancellationToken) -> NovaResult<Option<String>> {
    git_output(project_dir, &["rev-parse", "HEAD"], cancel)
}

/// Minor fix: a git command failure is no longer collapsed into a single
/// undifferentiated `None` — callers that care can distinguish "this
/// project directory isn't a git repository at all" from "the project
/// directory doesn't exist" from "git itself failed for some other reason"
/// (timeout, permission, crashed, ...). `git_output`/`git_status_porcelain`/
/// `git_head` still return `Option<String>` for their existing callers
/// (`None` on any failure, preserving the already-tested control flow
/// exactly), but now derive that from this classified result rather than a
/// single collapsed match arm.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GitOutputOutcome {
    Success(String),
    /// `project_dir` exists but is not (or is no longer) a git repository.
    NotAGitRepository,
    /// `project_dir` itself could not be used as a working directory (moved,
    /// deleted, or otherwise inaccessible).
    ProjectDirectoryMissing,
    /// Git ran (or tried to) but failed for a reason other than the two
    /// above — timeout, permission denied, unexpected non-zero exit, etc.
    ExecutionFailed(String),
}

fn git_output_classified(
    project_dir: &Path,
    args: &[&str],
    cancel: &CancellationToken,
) -> GitOutputOutcome {
    let request = ExecutionRequest::new(
        "git".to_string(),
        args.iter().map(|s| s.to_string()).collect(),
        project_dir.to_path_buf(),
        Some(project_dir.to_path_buf()),
    )
    .with_timeout(GIT_TIMEOUT);
    let result = process::execute_blocking(request, cancel.clone());
    match result.outcome {
        ExecutionOutcome::Success => GitOutputOutcome::Success(result.stdout),
        ExecutionOutcome::Failed { kind, detail } => match kind {
            ExecutionErrorKind::PathRejected | ExecutionErrorKind::NotFound => {
                GitOutputOutcome::ProjectDirectoryMissing
            }
            _ => {
                let combined = format!("{}\n{}", result.stdout, result.stderr).to_lowercase();
                if combined.contains("not a git repository") {
                    GitOutputOutcome::NotAGitRepository
                } else {
                    GitOutputOutcome::ExecutionFailed(redact_sensitive_text(&detail))
                }
            }
        },
    }
}

fn git_output(
    project_dir: &Path,
    args: &[&str],
    cancel: &CancellationToken,
) -> NovaResult<Option<String>> {
    match git_output_classified(project_dir, args, cancel) {
        GitOutputOutcome::Success(stdout) => Ok(Some(stdout)),
        GitOutputOutcome::NotAGitRepository
        | GitOutputOutcome::ProjectDirectoryMissing
        | GitOutputOutcome::ExecutionFailed(_) => Ok(None),
    }
}

/// Writes the developer AI's task package: the confirmed requirement card
/// fields plus, for a fix pass, the prior round's redacted findings.
///
/// Phase 3 remediation (Blocker 4): lives under Nova's own app-owned data
/// directory (`<app-data>/task-packages/<project-id>/<task-id>/round-<n>/`),
/// never inside the user's project directory at all — the original
/// implementation wrote to `<project>/.nova/tasks/<task-id>/`, which the
/// Codex review flagged as an unvalidated, project-tree-adjacent path that a
/// crafted `task_id`/`project_id` could have escaped. `project_id`/`task_id`
/// are validated single path components (via `app_paths::safe_app_owned_dir`
/// — rejects `..`, separators, drive prefixes, UNC, reserved device names,
/// trailing dot/space, and any symlink/junction ancestor) and the write
/// itself is atomic (temp file + rename), so a concurrent write or a
/// mid-write crash can never leave a half-written package behind.
fn write_task_package(
    db: &Database,
    task_id: &str,
    project_id: &str,
    round: i64,
    prior_findings: Option<&str>,
    app_data_dir: &Path,
) -> NovaResult<PathBuf> {
    let task = db.task(task_id)?;
    let dir = task_docs_dir(app_data_dir, project_id, task_id, round)?;
    let path = dir.join(if round == 1 {
        "REQUIREMENT.md".to_string()
    } else {
        format!("FIX_REQUEST_{round}.md")
    });

    let mut body = String::new();
    body.push_str(&format!("# Nova Task Package (round {round})\n\n"));
    body.push_str("You are the developer AI for this task. Read this file fully before making any change.\n\n");
    body.push_str(&format!(
        "## Goal\n\n{}\n\n",
        task.goal.as_deref().unwrap_or("(not specified)")
    ));
    body.push_str(&format!(
        "## In scope this round\n\n{}\n\n",
        task.in_scope.as_deref().unwrap_or("(not specified)")
    ));
    body.push_str(&format!(
        "## Explicitly not in scope\n\n{}\n\n",
        task.not_in_scope.as_deref().unwrap_or("(not specified)")
    ));
    body.push_str(&format!(
        "## Done means\n\n{}\n\n",
        task.done_criteria.as_deref().unwrap_or("(not specified)")
    ));
    if let Some(findings) = prior_findings {
        body.push_str("## Reviewer findings from the previous round — fix exactly these, do not expand scope\n\n");
        body.push_str(findings);
        body.push('\n');
    }
    body.push_str("\nDo not expand this task's scope beyond what is written above. Do not touch files unrelated to this task.\n");

    crate::app_paths::atomic_write(&path, body.as_bytes()).map_err(NovaError::from)?;
    Ok(path)
}

/// Writes the reviewer AI's package: requirement context, an explicit
/// read-only instruction (Nova's isolated workspace is the real technical
/// enforcement — see `review_isolation` — this text is a courtesy, not the
/// safety boundary itself), and the Major 2 structured-JSON verdict schema
/// the reviewer must echo back (with the exact `task_id`/
/// `result_generation`/`round` this round expects — see
/// `runtime::verdict::parse_structured_verdict`). Same app-owned-directory
/// placement as [`write_task_package`] — see Blocker 4 above.
fn write_review_package(
    db: &Database,
    task_id: &str,
    project_id: &str,
    round: i64,
    app_data_dir: &Path,
    test_summary: Option<&str>,
) -> NovaResult<PathBuf> {
    let task = db.task(task_id)?;
    let dir = task_docs_dir(app_data_dir, project_id, task_id, round)?;
    let path = dir.join(format!("REVIEW_REQUEST_{round}.md"));

    let mut body = String::new();
    body.push_str(&format!("# Nova Review Package (round {round})\n\n"));
    body.push_str("You are the independent reviewer AI for this task. You are READ-ONLY: do not modify, create, or delete any file. You are working inside a disposable copy of the project, not the original.\n\n");
    body.push_str(&format!(
        "## Original goal\n\n{}\n\n",
        task.goal.as_deref().unwrap_or("(not specified)")
    ));
    body.push_str(&format!(
        "## Acceptance criteria\n\n{}\n\n",
        task.done_criteria.as_deref().unwrap_or("(not specified)")
    ));
    // Blocker 3: the reviewer sees Nova's own independently-collected test
    // evidence, never the developer AI's self-report — this text is built
    // exclusively from `run_tests_for_round`'s real, just-executed outcome.
    body.push_str(&format!(
        "## Independent test evidence (collected by Nova, not the developer AI)\n\n{}\n\n",
        test_summary.unwrap_or("(no test evidence collected)")
    ));
    body.push_str("Review the actual code changes and any test evidence you can observe. End your entire output with exactly one fenced JSON code block (and nothing else outside it), containing exactly this object shape:\n\n");
    body.push_str("```json\n{\n");
    body.push_str("  \"schema_version\": 1,\n");
    body.push_str(&format!("  \"task_id\": \"{task_id}\",\n"));
    body.push_str(&format!(
        "  \"result_generation\": {},\n",
        task.result_generation
    ));
    body.push_str(&format!("  \"round\": {round},\n"));
    body.push_str("  \"verdict\": \"approve | changes_required | insufficient_evidence\",\n");
    body.push_str("  \"summary\": \"one paragraph explaining the verdict\",\n");
    body.push_str("  \"findings\": [\"specific issue 1\", \"specific issue 2\"]\n");
    body.push_str("}\n```\n");

    crate::app_paths::atomic_write(&path, body.as_bytes()).map_err(NovaError::from)?;
    Ok(path)
}

/// `<app-data>/task-packages/<project-id>/<task-id>/round-<n>/` — every
/// component individually validated by `app_paths::safe_app_owned_dir`
/// (never accepts `..`, a separator, a drive/UNC prefix, a reserved Windows
/// device name, a trailing dot/space, or a symlink/junction ancestor), and
/// the final resolved path is verified to still be inside the app-owned
/// `task-packages` root before it is ever returned. `round` is folded into
/// the directory itself (not just the file name) so a concurrent/second
/// generation's package can never partially overwrite a still-referenced
/// earlier round's files.
fn task_docs_dir(
    app_data_dir: &Path,
    project_id: &str,
    task_id: &str,
    round: i64,
) -> NovaResult<PathBuf> {
    let round_component = format!("round-{round}");
    crate::app_paths::safe_app_owned_dir(
        &app_data_dir.join("task-packages"),
        &[project_id, task_id, &round_component],
    )
    .map_err(NovaError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        ActionStatus, ActionStatus as ActStatus, CreateProjectRequest, CreateTaskRequest,
    };
    use crate::runtime::ai_adapter::mock::{MockAdapter, MockOutcome};
    use std::process::Command as StdCommand;
    use std::sync::Arc;
    use std::thread;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-core-loop-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// A fresh app-owned data directory for a single test's task packages —
    /// mirrors the real `AppState.app_data_dir` every production call
    /// passes to `run_core_loop`.
    fn test_app_data_dir() -> PathBuf {
        temp_dir("app-data")
    }

    /// A real, minimal git repository with one committed file — needed so
    /// `git status --porcelain`/`git rev-parse HEAD` behave like a real
    /// project rather than erroring on an empty/non-git directory.
    fn git_project_dir(name: &str) -> PathBuf {
        let dir = temp_dir(name);
        for args in [
            vec!["init", "-q"],
            // Real bug found via re-review testing: OS temp directory
            // names here are already long (`nova-core-loop-test-<name>-
            // <pid>-<uuid>`), and a `.git/objects/<2-hex>/<38-hex>` path
            // underneath one can exceed Windows' default 260-character
            // `MAX_PATH`, making git itself fail with "unable to write
            // file ... Filename too long" / "fatal: adding files failed"
            // — noisy stderr on *every* full test-suite run, even though
            // the specific object usually still lands (git retries/other
            // objects succeed) so the test's own assertions rarely
            // noticed. `core.longpaths` makes git use the `\\?\` extended-
            // path Win32 API internally, removing the limit entirely
            // rather than working around it by guessing at a path budget.
            vec!["config", "core.longpaths", "true"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Nova Test"],
        ] {
            StdCommand::new("git")
                .args(&args)
                .current_dir(&dir)
                .status()
                .expect("git setup");
        }
        fs::write(dir.join("README.md"), "hello").unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(&dir)
            .status()
            .unwrap();
        dir
    }

    /// Creates a real project + confirmed task against an *already-open*
    /// `Database` — shared by [`db_with_confirmed_task`] (fresh in-memory
    /// database) and any test that instead needs a real file-backed
    /// database it can close and reopen (e.g. to prove a completed task's
    /// memory/evidence survive a genuine process restart).
    fn create_confirmed_task_in(db: &Database, dir: &Path, dev_ai_kind: &str) -> (String, String) {
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
                    title: "Add search box".to_string(),
                    goal: Some("给列表加搜索".to_string()),
                    in_scope: Some("搜索框".to_string()),
                    not_in_scope: Some("分页".to_string()),
                    done_criteria: Some("能按关键字搜索".to_string()),
                    ai_kind: Some(dev_ai_kind.to_string()),
                },
                Actor::User,
            )
            .unwrap();
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
        (project.id, task.id)
    }

    fn db_with_confirmed_task(dir: &Path, dev_ai_kind: &str) -> (Database, String, String) {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let (project_id, task_id) = create_confirmed_task_in(&db, dir, dev_ai_kind);
        (db, project_id, task_id)
    }

    /// Most fixtures in this test module use a bare `git_project_dir` with
    /// no detectable test framework at all — deliberately, so tests that
    /// are actually about the dev/review/cancellation flow are not slowed
    /// down by a real test run. Since the second re-review (Blocker 3), a
    /// missing test command only ever becomes `no_tests_applicable`
    /// evidence behind a real, persisted user confirmation (see
    /// `core_loop::run_tests_for_round`), so every such fixture-driven test
    /// that needs the loop to get *past* the test-execution step now calls
    /// this first, for exactly the generation/round(s) it needs.
    ///
    /// Codex re-review remediation (Major 2): a bare `git_project_dir` has
    /// neither `Cargo.toml` nor `package.json`, which `detect_build_command`
    /// now correctly classifies as `Insufficient` (Nova cannot determine
    /// whether a build applies), not `NotApplicable` — so getting *past*
    /// the build step for the same fixture also now requires a real,
    /// persisted `no_build_confirmation` for the same generation/round(s).
    /// This helper records both in one call, since every existing caller
    /// needs both together.
    fn confirm_no_tests(
        db: &Database,
        project_id: &str,
        task_id: &str,
        generation: i64,
        rounds: &[i64],
    ) {
        for round in rounds {
            db.record_no_tests_confirmation(project_id, task_id, generation, *round, None)
                .unwrap();
            db.record_no_build_confirmation(project_id, task_id, generation, *round, None)
                .unwrap();
        }
    }

    // --- Work Package 3: full single-AI closed loop, either developer AI --
    //
    // `CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md` 工作包 3 / `docs/
    // release-v0.1.0/09_V0.1_SCOPE_CORRECTION09241121.md`: proves the
    // *entire* v0.1.0 P0 main path end to end for a single connected
    // developer AI — real git project, real `npm test`/`npm run build`
    // fixture commands (through Phase 1's real process boundary, not
    // faked), real Action Ledger/evidence/document generation, real
    // acceptance, real `Completed`, and a real file-backed database
    // close+reopen to prove the result survives an actual process
    // restart. Only the AI CLI process itself is a test double
    // (`MockAdapter`, implementing the same real `DevReviewAdapter`
    // boundary a real Codex/Claude adapter does) — every other boundary
    // (storage, git, test/build execution, document generation) is real
    // production code.
    fn codex_or_claude_only_full_closed_loop_reaches_completed_and_survives_reopen(
        dev_ai_kind: &str,
        fixture_name: &str,
    ) {
        let dir = git_project_dir(fixture_name);
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\"","build":"node -e \"process.exit(0)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add passing test+build scripts"])
            .current_dir(&dir)
            .status()
            .unwrap();

        let db_dir = temp_dir(&format!("{fixture_name}-db"));
        let db_path = db_dir.join("nova.sqlite3");
        let app_data_dir = db_dir.join("app-data");

        let (project_id, task_id) = {
            let db = Database::open(&db_path).unwrap();
            db.migrate().unwrap();
            let (project_id, task_id) = create_confirmed_task_in(&db, &dir, dev_ai_kind);

            // The unselected AI is never referenced anywhere in this test
            // at all — no readiness probe, no binary path, nothing —
            // which is itself the strongest possible proof that it cannot
            // block this closed loop: there is nothing here *to* block.
            let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);

            let outcome = run_core_loop(
                &db,
                &task_id,
                &adapter,
                &CancellationToken::new(),
                &app_data_dir,
            )
            .unwrap();
            assert_eq!(
                outcome.task.state,
                TaskState::AwaitingUserAcceptance,
                "real dev + real test + real build must reach awaiting_user_acceptance"
            );
            assert_eq!(
                adapter.development_call_count(),
                1,
                "exactly one development invocation for a clean first round"
            );
            assert_eq!(
                adapter.review_call_count(),
                0,
                "the v0.1.0 single-AI main path must never invoke a reviewer"
            );
            assert!(
                db.list_task_reviews(&project_id, &task_id)
                    .unwrap()
                    .is_empty(),
                "no task_reviews row may exist for a task completed via the single-AI path"
            );

            let accepted = accept_task_result(
                &db,
                &project_id,
                &task_id,
                true,
                Some("looks good"),
                &app_data_dir,
            )
            .unwrap();
            assert_eq!(accepted.state, TaskState::Completed);

            let final_result_path = app_data_dir
                .join("result-documents")
                .join(&project_id)
                .join(&task_id)
                .join("FINAL_RESULT.md");
            let content = fs::read_to_string(&final_result_path).unwrap();
            assert!(
                content.contains("已完成（用户已验收）"),
                "FINAL_RESULT.md must show Completed from Nova's own verification and a real \
                 user acceptance alone: {content}"
            );

            (project_id, task_id)
        }; // `db` (and its one real sqlite connection) is dropped here.

        // Real process-restart proof: open a brand new `Database` against
        // the same file and confirm the task's final state, evidence, and
        // acceptance are all still there — nothing lived only in memory.
        let reopened = Database::open(&db_path).unwrap();
        let task = reopened.task(&task_id).unwrap();
        assert_eq!(task.state, TaskState::Completed);
        assert_eq!(task.result_generation, 1);
        let test_evidence = reopened
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(test_evidence.len(), 1);
        assert_eq!(test_evidence[0].status, "passed");
        let build_evidence = reopened
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(build_evidence.len(), 1);
        assert_eq!(build_evidence[0].status, "passed");
        assert!(reopened
            .list_task_reviews(&project_id, &task_id)
            .unwrap()
            .is_empty());
        let final_result_path = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id)
            .join("FINAL_RESULT.md");
        let content_after_reopen = fs::read_to_string(&final_result_path).unwrap();
        assert!(content_after_reopen.contains("已完成（用户已验收）"));

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[test]
    fn codex_only_full_closed_loop_reaches_completed_and_survives_a_real_database_reopen() {
        codex_or_claude_only_full_closed_loop_reaches_completed_and_survives_reopen(
            "codex_cli",
            "codex-only-full-closed-loop",
        );
    }

    #[test]
    fn claude_only_full_closed_loop_reaches_completed_and_survives_a_real_database_reopen() {
        codex_or_claude_only_full_closed_loop_reaches_completed_and_survives_reopen(
            "claude_cli",
            "claude-only-full-closed-loop",
        );
    }

    // --- v0.1.0 scope correction (`CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`):
    // single-AI main path tests. `run_core_loop` (unlike the dormant
    // `run_core_loop_dual_ai_v0_2` the tests further below call) never
    // invokes a reviewer at all — these prove the real v0.1.0 product path:
    // one developer AI -> Nova's own independent verification -> at most
    // one same-AI fix pass -> preview_ready/awaiting_user_acceptance, with
    // no second AI ever probed, invoked, or required.

    #[test]
    fn single_ai_main_flow_reaches_awaiting_user_acceptance_with_no_reviewer_ever_invoked() {
        let dir = git_project_dir("single-ai-main-flow");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        // Even though the mock *would* approve if asked, `run_core_loop`
        // must never ask — see the assertion on `review_call_count()`
        // below.
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let app_data_dir = test_app_data_dir();

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        assert_eq!(adapter.development_call_count(), 1);
        assert_eq!(
            adapter.review_call_count(),
            0,
            "the v0.1.0 single-AI main path must never invoke a reviewer, ready or not"
        );
        let reviews = db.list_task_reviews(&project_id, &task_id).unwrap();
        assert!(
            reviews.is_empty(),
            "no task_reviews row may exist for a task that completed via the single-AI path"
        );

        let final_result_path = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id)
            .join("FINAL_RESULT.md");
        let content = fs::read_to_string(&final_result_path).unwrap();
        assert!(
            content.contains("等待用户验收"),
            "FINAL_RESULT.md must show ready-for-acceptance from Nova's own verification alone, \
             never gated on a reviewer row that does not exist: {content}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_ai_one_fix_pass_by_the_same_developer_ai_then_success() {
        // A genuine, real independent test failure — never a reviewer
        // verdict — triggers the one allowed automatic fix pass, run by
        // the *same* developer AI, then Nova re-verifies for real.
        let dir = git_project_dir("single-ai-fix-then-pass");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(1)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add failing test"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        // Round 1's real `npm test` genuinely fails (exit 1); round 2's dev
        // side effect rewrites `package.json` so the *same* command
        // genuinely passes — simulating the same developer AI's fix,
        // re-verified for real by Nova, never assumed.
        let dir_for_fix = dir.clone();
        let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);
        let call_count = std::sync::atomic::AtomicUsize::new(0);
        *adapter.development_side_effect.lock().unwrap() = Some(Box::new(move |_req| {
            if call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                fs::write(
                    dir_for_fix.join("package.json"),
                    r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\""}}"#,
                )
                .unwrap();
            }
        }));

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(
            outcome.task.state,
            TaskState::AwaitingUserAcceptance,
            "round 2's genuinely-passing re-verification must succeed the task"
        );
        assert_eq!(
            adapter.development_call_count(),
            2,
            "exactly one fix pass means exactly two development invocations, by the same AI"
        );
        assert_eq!(adapter.review_call_count(), 0);
        let round1 = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        let round2 = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 2)
            .unwrap();
        assert_eq!(round1.len(), 1);
        assert_eq!(round1[0].status, "failed");
        assert_eq!(round2.len(), 1);
        assert_eq!(round2[0].status, "passed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_ai_one_fix_pass_by_claude_code_as_the_developer_ai_then_success() {
        // Work Package 3: the same one-fix-pass-then-pass proof as above,
        // but with Claude Code selected as the developer AI instead of
        // Codex — proves the fix-pass behavior is not accidentally
        // specific to one CLI kind.
        let dir = git_project_dir("single-ai-fix-then-pass-claude");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(1)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add failing test"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "claude_cli");
        let dir_for_fix = dir.clone();
        let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);
        let call_count = std::sync::atomic::AtomicUsize::new(0);
        *adapter.development_side_effect.lock().unwrap() = Some(Box::new(move |_req| {
            if call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                fs::write(
                    dir_for_fix.join("package.json"),
                    r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\""}}"#,
                )
                .unwrap();
            }
        }));

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(
            outcome.task.state,
            TaskState::AwaitingUserAcceptance,
            "round 2's genuinely-passing re-verification must succeed the task"
        );
        assert_eq!(adapter.development_call_count(), 2);
        assert_eq!(adapter.review_call_count(), 0);
        let round1 = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        let round2 = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 2)
            .unwrap();
        assert_eq!(round1.len(), 1);
        assert_eq!(round1[0].status, "failed");
        assert_eq!(round2.len(), 1);
        assert_eq!(round2[0].status, "passed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_ai_second_verification_still_failing_stops_at_failed() {
        // Same real-failing-test setup as above, but round 2's dev side
        // effect changes nothing — the same command genuinely fails again,
        // and the task must stop at `failed`, never silently retry a third
        // time.
        let dir = git_project_dir("single-ai-fix-still-fails");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(1)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add failing test"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);
        let app_data_dir = test_app_data_dir();

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::Failed);
        assert_eq!(
            adapter.development_call_count(),
            2,
            "still exactly one fix pass — never a second automatic retry"
        );
        assert_eq!(adapter.review_call_count(), 0);
        let final_result_path = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id)
            .join("FINAL_RESULT.md");
        let content = fs::read_to_string(&final_result_path).unwrap();
        assert!(content.contains("未通过"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_ai_second_verification_still_failing_stops_at_failed_with_claude_code_as_developer() {
        // Work Package 3: the same "second attempt still genuinely fails,
        // never a third automatic try" proof, with Claude Code as the
        // developer AI.
        let dir = git_project_dir("single-ai-fix-still-fails-claude");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(1)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add failing test"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "claude_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);
        let app_data_dir = test_app_data_dir();

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::Failed);
        assert_eq!(
            adapter.development_call_count(),
            2,
            "still exactly one fix pass — never a second automatic retry"
        );
        assert_eq!(adapter.review_call_count(), 0);
        let final_result_path = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id)
            .join("FINAL_RESULT.md");
        let content = fs::read_to_string(&final_result_path).unwrap();
        assert!(content.contains("未通过"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_ai_resume_after_dev_already_succeeded_does_not_repeat_development() {
        // "应用恢复单 AI 任务时不重复开发" — pre-seed the ledger as if round
        // 1's development already completed successfully in an earlier,
        // uninterrupted attempt (the crash-recovery shape), then resume via
        // the real single-AI `run_core_loop` entry point and confirm the
        // developer AI is never invoked again for that round.
        let dir = git_project_dir("single-ai-resume-no-repeat-dev");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let dev_key = format!("dev-{task_id}-round-1");
        db.plan_action(&task_id, "ai_development", &dev_key)
            .unwrap();
        db.start_action(&task_id, &dev_key, false).unwrap();
        db.finish_action(&task_id, &dev_key, ActionStatus::Succeeded, None, None)
            .unwrap();
        // Simulate the real crash-recovery shape: `running -> interrupted`
        // (what `Database::interrupt_all_in_flight_tasks` does on startup),
        // then `interrupted -> queued` (the real "user resumes" edge) —
        // matching the actual legal state machine rather than skipping
        // straight from `running` to `queued`, which is not a legal edge.
        db.transition_task(
            &task_id,
            TaskState::Interrupted,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        assert_eq!(
            adapter.development_call_count(),
            0,
            "development already succeeded before the crash — resuming must not repeat it, \
             let alone re-invoke a paid AI call"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_dual_ai_task_reviews_rows_are_read_safely_and_never_required_by_the_single_ai_path() {
        // "旧双 AI 任务记录可以被安全读取" — a `task_reviews` row written under
        // the old (or dormant v0.2.0) dual-AI path must not crash anything
        // when read back, and must not become a hidden requirement for a
        // single-AI task's own completion.
        let dir = git_project_dir("old-dual-ai-data-compat");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        db.record_task_review(
            &project_id,
            &task_id,
            0,
            1,
            "claude_cli",
            "changes_required",
            Some("old dual-AI era finding"),
        )
        .unwrap();

        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(
            outcome.task.state,
            TaskState::AwaitingUserAcceptance,
            "a stale changes_required review row must never block a single-AI task's own \
             completion — the single-AI path does not even look at it"
        );
        let reviews = db.list_task_reviews(&project_id, &task_id).unwrap();
        assert_eq!(
            reviews.len(),
            1,
            "the old row must still be safely readable"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Codex re-review remediation (Major 3): independent build gate --

    /// v0.1.0 scope correction (Work Package 2 final stability
    /// remediation): the marker-wait budget for the two cancellation tests
    /// below (`cancelling_while_the_independent_build_is_running_is_
    /// recorded_as_cancelled_not_failed` and `cancelling_while_
    /// independent_tests_are_running_is_recorded_as_cancelled_not_failed`).
    ///
    /// Root-cause evidence (not a guess): reaching the point where the
    /// spawned child's own script runs its first statement requires, on
    /// Windows, `runtime::win_spawn::resolve_executable` to walk every
    /// `PATH` entry x `PATHEXT` looking for `npm`/`cmd.exe` (`npm` ships as
    /// `npm.cmd`, a batch script `CreateProcessW` cannot launch directly —
    /// see `win_spawn::is_batch_script` — so `cmd.exe` becomes a real,
    /// separate hop), then `runtime::job_object`/`runtime::process::
    /// run_windows` creating a suspended process, creating and configuring
    /// a Job Object, assigning the child to it, and only then resuming
    /// it — all *before* `npm` (itself a real Node.js program with its own
    /// CLI bootstrap cost) has even spawned the actual `test`/`build`
    /// script as a grandchild. Under a full parallel `cargo test
    /// --workspace` run, with potentially hundreds of other test threads
    /// doing the same PATH walks, process creations, and Job Object calls
    /// at the same moment (compounded by antivirus real-time scanning of
    /// freshly-spawned executables, a common source of extra latency this
    /// exact chain is exposed to on every hop), the prior fixed 10-second
    /// budget was measured to be insufficient — see the "PHASE3_BUILD_
    /// CANCELLATION_STABILITY_REMEDIATION_REPORT" for the reproduction
    /// (30 consecutive single-test runs of the directed test never failed;
    /// a genuine full-suite parallel run reproduced the timeout). This is
    /// environmental/process-creation latency under load, not a defect in
    /// Nova's own cancellation logic (which polls at a fixed 25ms interval
    /// once the process is actually running — see `runtime::process::
    /// POLL_INTERVAL`) or in the marker signal itself (a real event written
    /// by the child, never a fixed sleep).
    const MARKER_WAIT_TIMEOUT: Duration = Duration::from_secs(45);

    /// Bounded wait for a real signal a spawned subprocess writes to disk —
    /// used instead of a fixed-duration sleep so cancellation tests never
    /// race against CPU-scheduling delays under full-suite parallel load.
    /// Panics (failing the test loudly, with as much diagnosis as can be
    /// safely gathered from the filesystem alone — see the caller for the
    /// additional Action Ledger/task-state diagnostics gathered *after*
    /// this panic propagates, once it is safe to touch `db` again) rather
    /// than waiting unboundedly if the signal never arrives.
    fn wait_for_marker_file(path: &Path, dir: &Path, timeout: Duration) {
        let started = std::time::Instant::now();
        let deadline = started + timeout;
        while !path.exists() {
            if std::time::Instant::now() >= deadline {
                let dir_listing = fs::read_dir(dir)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .map(|e| e.file_name().to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_else(|e| format!("<failed to list dir: {e}>"));
                panic!(
                    "marker file never appeared within {timeout:?} (waited {:?})\n  \
                     marker path: {}\n  \
                     project fixture dir: {} (exists: {}, entries: [{dir_listing}])",
                    started.elapsed(),
                    path.display(),
                    dir.display(),
                    dir.exists(),
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// v0.1.0 scope correction (Work Package 2 final stability
    /// remediation): Action Ledger/task-state diagnostics for the two
    /// marker-wait cancellation tests — deliberately gathered from the
    /// *main* test thread only, after `canceller.join()` has already
    /// returned (whether the canceller thread itself panicked or
    /// succeeded), never concurrently from the canceller thread itself.
    /// `Database`'s underlying SQLite connection is not safe to read from
    /// a second thread while `run_core_loop` may still be mid-flight
    /// writing to it on the main thread, and the in-memory fixture
    /// database these tests use (`db_with_confirmed_task` ->
    /// `Database::memory()`) has no on-disk file a second connection could
    /// even reopen. Never includes environment variables, full command
    /// lines, or anything that could leak a secret — only a path, a
    /// task-state enum value, and an Action Ledger status/attempt count,
    /// plus the action's own already-redacted `error_summary` (see
    /// `security::redact_sensitive_text`, applied before it was ever
    /// written to the Action Ledger).
    fn marker_wait_action_diagnostics(db: &Database, task_id: &str, action_key: &str) -> String {
        let task_state = db
            .task(task_id)
            .map(|t| t.state.to_string())
            .unwrap_or_else(|e| format!("<failed to read task: {e}>"));
        let action_status = match db.find_action(task_id, action_key) {
            Ok(Some(action)) => format!(
                "{:?} (attempt {}, error_summary: {:?})",
                action.status, action.attempt_number, action.error_summary
            ),
            Ok(None) => {
                "<no action row yet — the subprocess may never have been spawned>".to_string()
            }
            Err(e) => format!("<failed to read action: {e}>"),
        };
        format!(
            "  task_id: {task_id}\n  current task state: {task_state}\n  action `{action_key}` \
             status: {action_status}"
        )
    }

    /// v0.1.0 scope correction (Work Package 2 final stability
    /// remediation): joins a marker-wait canceller thread and, if it
    /// panicked, re-panics on the main thread with the original message
    /// plus fresh Action Ledger/task-state diagnostics — safe to read now
    /// because the canceller thread has fully finished and `run_core_loop`
    /// has already returned by the time callers reach this.
    fn join_canceller_with_diagnostics(
        canceller: std::thread::JoinHandle<()>,
        db: &Database,
        task_id: &str,
        action_key: &str,
    ) {
        if let Err(panic_payload) = canceller.join() {
            let original = panic_payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic_payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            panic!(
                "{original}\n  additional diagnostics gathered after the panic (safe to read \
                 now that run_core_loop has already returned):\n{}",
                marker_wait_action_diagnostics(db, task_id, action_key)
            );
        }
    }

    /// v0.1.0 scope correction (Work Package 2 final stability
    /// remediation): after a cancellation test terminates the build/test
    /// subprocess via the Windows Job Object
    /// (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), actual handle release can
    /// lag the kill signal by a brief moment. Retries `fs::remove_dir_all`
    /// for a few seconds instead of silently swallowing a failure (the
    /// `let _ = fs::remove_dir_all(...)` pattern used elsewhere in this
    /// file for fixtures that never had a subprocess holding anything
    /// open) — a deletion that keeps failing past this grace period is
    /// itself evidence of a genuinely leaked child process still holding a
    /// file handle inside the directory, and is surfaced as a test
    /// failure rather than ignored.
    fn remove_dir_all_verifying_no_leftover_handles(dir: &Path) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match fs::remove_dir_all(dir) {
                Ok(()) => return,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => panic!(
                    "failed to remove the project fixture directory after the test finished — \
                     a leftover child process may still be holding an open handle inside it: \
                     {dir:?}: {e}"
                ),
            }
        }
    }

    #[test]
    fn wait_for_marker_file_times_out_with_rich_diagnostics_when_the_marker_never_appears() {
        // v0.1.0 scope correction (Work Package 2 final stability
        // remediation), required test coverage item 5 ("子进程提前退出时测试
        // 快速失败并输出原因"): exercises `wait_for_marker_file`'s panic path
        // directly and deterministically — no real subprocess, no real
        // timing dependency beyond a short, fixed budget for *this specific
        // helper unit test* (distinct from `MARKER_WAIT_TIMEOUT`, which is
        // sized for real npm/node process-creation latency under full-suite
        // load, not for this isolated check) — and asserts the panic
        // message actually contains every diagnostic field required by the
        // stability remediation report.
        let dir = temp_dir("marker-wait-diagnostics");
        let marker = dir.join("never-appears.marker");
        fs::write(dir.join("sentinel.txt"), "present").unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wait_for_marker_file(&marker, &dir, Duration::from_millis(200));
        }));

        let err = result.expect_err("must panic when the marker never appears");
        let message = err
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
            .expect("panic payload must be a string message");
        assert!(
            message.contains("marker file never appeared"),
            "message must state what happened: {message}"
        );
        assert!(
            message.contains(&marker.to_string_lossy().into_owned()),
            "message must contain the full marker path: {message}"
        );
        assert!(
            message.contains(&dir.to_string_lossy().into_owned()),
            "message must contain the project fixture directory: {message}"
        );
        assert!(
            message.contains("sentinel.txt"),
            "message must list the fixture directory's actual contents, proving it really \
             inspected the filesystem rather than guessing: {message}"
        );
        assert!(
            message.contains("waited"),
            "message must report how long it actually waited: {message}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_fixture_directories_are_unique_even_for_the_same_requested_name() {
        // v0.1.0 scope correction (Work Package 2 final stability
        // remediation), required test coverage item 2 ("marker路径在并行测试
        // 之间唯一"): `git_project_dir`/`temp_dir` embed the current process
        // id and a fresh UUID in every generated path — this proves two
        // fixtures created with the *identical* literal name (exactly what
        // two parallel test-harness threads running the *same* test
        // function concurrently, or a test re-run, would pass) can never
        // collide, so a marker file from one can never be mistaken for, or
        // overwritten by, another.
        let dir_a = temp_dir("same-name-collision-check");
        let dir_b = temp_dir("same-name-collision-check");
        assert_ne!(
            dir_a, dir_b,
            "two fixtures requested with the same name must never resolve to the same directory"
        );
        assert!(
            !dir_a.join("build-started.marker").exists(),
            "sanity check: a fresh fixture directory must never already contain a marker file \
             left over from a different test"
        );
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn test_passed_and_build_passed_reaches_awaiting_user_acceptance() {
        let dir = git_project_dir("build-gate-both-pass");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\"","build":"node -e \"process.exit(0)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        let build_evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(build_evidence.len(), 1);
        assert_eq!(build_evidence[0].status, "passed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn round1_build_failure_triggers_the_same_dev_ais_one_fix_pass_and_round2_build_failure_ends_in_failed(
    ) {
        // Tests genuinely pass in both rounds; the build script always
        // fails — a real, independent way to exercise "tests passed but
        // build failed" without a self-contradictory fixture (unlike Rust,
        // an npm project's test/build scripts are genuinely independent
        // commands).
        let dir = git_project_dir("build-gate-always-fails");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\"","build":"node -e \"process.exit(1)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(
            outcome.task.state,
            TaskState::Failed,
            "a build failure that persists through the one automatic fix pass must end in failed, \
             never in awaiting_user_acceptance"
        );
        assert_eq!(
            adapter.development_call_count(),
            2,
            "round 1's build failure must trigger exactly one automatic fix pass by the same dev AI"
        );
        let round1_test = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(
            round1_test[0].status, "passed",
            "round 1's tests genuinely passed — only the build failed"
        );
        let round1_build = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(round1_build[0].status, "failed");
        let round2_build = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 2)
            .unwrap();
        assert_eq!(
            round2_build[0].status, "failed",
            "round 2 must produce its own fresh build evidence, not reuse round 1's"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_while_the_independent_build_is_running_is_recorded_as_cancelled_not_failed() {
        let dir = git_project_dir("build-gate-cancel-during-build");
        let marker = dir.join("build-started.marker");
        // v0.1.0 scope correction (Work Package 2 final stability
        // remediation): write to a temp name then rename, so any other
        // process polling for `build-started.marker` by existence alone
        // (as `wait_for_marker_file` does) can never observe a
        // partially-written file — the rename is atomic on the same
        // volume, and the target name only ever appears once fully
        // formed.
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\"","build":"node -e \"require('fs').writeFileSync('build-started.marker.tmp','1');require('fs').renameSync('build-started.marker.tmp','build-started.marker');setTimeout(()=>process.exit(0), 6000)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let build_key = build_action_key(&task_id, 0, 1);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        let cancel = CancellationToken::new();
        let cancel_for_timer = cancel.clone();
        let marker_for_timer = marker.clone();
        let dir_for_timer = dir.clone();
        let canceller = std::thread::spawn(move || {
            wait_for_marker_file(&marker_for_timer, &dir_for_timer, MARKER_WAIT_TIMEOUT);
            cancel_for_timer.cancel();
        });

        let outcome =
            run_core_loop(&db, &task_id, &adapter, &cancel, &test_app_data_dir()).unwrap();
        join_canceller_with_diagnostics(canceller, &db, &task_id, &build_key);

        assert_eq!(outcome.task.state, TaskState::Cancelled);
        let action = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(
            action.status,
            ActionStatus::Cancelled,
            "the build-execution action must be `cancelled`, not `failed`"
        );
        let evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].status, "cancelled");
        remove_dir_all_verifying_no_leftover_handles(&dir);
    }

    #[test]
    fn resuming_after_a_round_1_build_already_succeeded_does_not_repeat_the_build() {
        // Crash-recovery shape: round 1's build action already reached
        // `succeeded` (and real build_evidence was recorded) in an earlier
        // attempt — a resumed call must reuse that fact, never spawn a
        // second real build process for the same round.
        let dir = git_project_dir("build-gate-resume-no-repeat");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let build_key = build_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "build_execution", &build_key)
            .unwrap();
        db.start_action(&task_id, &build_key, false).unwrap();
        db.finish_action(&task_id, &build_key, ActionStatus::Succeeded, None, None)
            .unwrap();
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
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

        let result = run_build_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();

        assert!(matches!(result, BuildRoundOutcome::Passed(_)));
        let evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(
            evidence.len(),
            1,
            "resuming an already-succeeded build action must never record a second build_evidence row"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Codex re-review remediation (Major 2): unknown/unreadable build
    // detection must fail closed, never auto-pass -----------------------

    #[test]
    fn unrecognized_project_type_stalls_at_needs_user_action_without_confirmation() {
        let dir = git_project_dir("build-insufficient-no-confirmation");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        // Only confirm no-tests, deliberately not no-build — proves the
        // build gate's own confirmation requirement is independent of the
        // test gate's.
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(
            outcome.task.state,
            TaskState::NeedsUserAction,
            "an unrecognized project type must never auto-pass the build gate"
        );
        assert_ne!(outcome.task.state, TaskState::PreviewReady);
        assert_ne!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        let evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].status, "insufficient_evidence");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_confirming_no_build_applicable_unblocks_the_stalled_task_to_success() {
        let dir = git_project_dir("build-insufficient-then-confirmed");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        let app_data_dir = test_app_data_dir();

        let stuck = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();
        assert_eq!(stuck.task.state, TaskState::NeedsUserAction);

        // Codex re-review remediation (final Major): the real, secured
        // entry point — never the raw storage function plus a manual
        // unblock call.
        let confirmation =
            confirm_no_build_applicable(&db, &project_id, &task_id, Some("confirmed by user"))
                .unwrap();
        assert_eq!(confirmation.round, 1);
        assert_eq!(confirmation.result_generation, 0);
        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::Queued,
            "confirm_no_build_applicable must itself resume the task, not merely record the row"
        );

        let resumed = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();
        assert_eq!(
            resumed.task.state,
            TaskState::AwaitingUserAcceptance,
            "after a real no-build confirmation, the task must actually complete"
        );
        let evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        // Two real rows are expected here: the first stalled attempt
        // recorded `insufficient_evidence`, and the resumed attempt (after
        // the real confirmation) recorded `no_build_applicable` — both are
        // genuine, independently-recorded facts, never collapsed into one.
        assert_eq!(evidence.len(), 2);
        let latest = evidence.last().unwrap();
        assert_eq!(latest.status, "no_build_applicable");
        assert!(
            latest
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("用户已确认")),
            "the evidence must reference the real user confirmation, never just say passed: {:?}",
            latest.reason
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_no_build_confirmation_persists_across_a_fresh_database_connection_to_the_same_file() {
        // "重启仍有效" — the confirmation is a real SQL row committed to the
        // database file, not frontend in-memory state; re-opening a brand
        // new `Database` handle against the same file (simulating an app
        // restart) must still see it.
        let dir = temp_dir("build-confirmation-restart");
        let db_path = dir.join("nova.sqlite3");
        let db1 = Database::open(&db_path).unwrap();
        db1.migrate().unwrap();
        let project = db1
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
        let task = db1
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "T".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        db1.record_no_build_confirmation(&project.id, &task.id, 0, 1, Some("restart test"))
            .unwrap();
        drop(db1);

        let db2 = Database::open(&db_path).unwrap();
        let found = db2
            .find_no_build_confirmation(&project.id, &task.id, 0, 1)
            .unwrap();
        assert!(
            found.is_some(),
            "a no-build confirmation must survive a fresh connection to the same database file"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_no_build_confirmation_does_not_carry_forward_to_a_different_generation() {
        let dir = git_project_dir("build-confirmation-wrong-generation");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let found = db
            .find_no_build_confirmation(&project_id, &task_id, 1, 1)
            .unwrap();
        assert!(
            found.is_none(),
            "a generation-0 no-build confirmation must never silently cover generation 1"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Codex re-review remediation (final Major closing Work Package 1):
    // `confirm_no_build_applicable`'s full authorization/state/round/
    // idempotency boundary. -------------------------------------------

    /// A real, genuine round-1 build-insufficient stall: bare git fixture
    /// (no `Cargo.toml`/`package.json`), no-tests confirmed for round 1 so
    /// the loop actually reaches the build step, then run to completion —
    /// mirrors `unrecognized_project_type_stalls_at_needs_user_action_
    /// without_confirmation` exactly.
    fn stall_task_at_round_1_build_insufficient(
        name: &str,
        dev_kind: &str,
    ) -> (PathBuf, Database, String, String) {
        let dir = git_project_dir(name);
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, dev_kind);
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(outcome.task.state, TaskState::NeedsUserAction);
        (dir, db, project_id, task_id)
    }

    /// A real, genuine round-2 build-insufficient stall, constructed
    /// directly at the Action Ledger level (round 1 fully succeeded — dev,
    /// test, and build, each a real terminal `Succeeded` action with
    /// matching evidence — round 2's dev and test also already succeeded)
    /// then a real call to `run_build_for_round(round=2)` against the same
    /// bare fixture (no `Cargo.toml`/`package.json` ever existed) so
    /// `detect_build_command` genuinely returns `Insufficient` for round 2
    /// specifically. Avoids re-running the full two-round core loop
    /// orchestration end to end (already covered by other tests) while
    /// still exercising a real, non-fabricated stall for the authorization
    /// boundary tests to target.
    fn stall_task_at_round_2_build_insufficient(
        name: &str,
        dev_kind: &str,
    ) -> (PathBuf, Database, String, String) {
        let dir = git_project_dir(name);
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, dev_kind);
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        for (kind, key) in [
            ("ai_development", format!("dev-{task_id}-round-1")),
            ("test_execution", test_action_key(&task_id, 0, 1)),
        ] {
            db.plan_action(&task_id, kind, &key).unwrap();
            db.start_action(&task_id, &key, false).unwrap();
            db.finish_action(&task_id, &key, ActionStatus::Succeeded, None, None)
                .unwrap();
        }
        db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
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
        let build_key_r1 = build_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "build_execution", &build_key_r1)
            .unwrap();
        db.start_action(&task_id, &build_key_r1, false).unwrap();
        db.finish_action(&task_id, &build_key_r1, ActionStatus::Succeeded, None, None)
            .unwrap();
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
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

        for (kind, key) in [
            ("ai_development", format!("dev-{task_id}-round-2")),
            ("test_execution", test_action_key(&task_id, 0, 2)),
        ] {
            db.plan_action(&task_id, kind, &key).unwrap();
            db.start_action(&task_id, &key, false).unwrap();
            db.finish_action(&task_id, &key, ActionStatus::Succeeded, None, None)
                .unwrap();
        }
        db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 2,
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
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let outcome = run_build_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            2,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(matches!(outcome, BuildRoundOutcome::ActionUncertain));
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);
        assert_eq!(
            current_pending_round(&db, &project_id, &task_id).unwrap(),
            2,
            "sanity check: this fixture must genuinely compute round 2"
        );
        (dir, db, project_id, task_id)
    }

    // --- v0.1.0 scope correction (Work Package 2): the read-only
    // `no_build_confirmation_needed` query the frontend uses to decide
    // whether to show the "confirm no build applies" prompt. ---------------

    #[test]
    fn no_build_confirmation_needed_is_true_for_a_genuine_build_insufficient_stall() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nbc-needed-true", "codex_cli");
        assert!(no_build_confirmation_needed(&db, &project_id, &task_id).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_build_confirmation_needed_is_false_before_any_stall() {
        let dir = git_project_dir("nbc-needed-false-fresh");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        assert!(!no_build_confirmation_needed(&db, &project_id, &task_id).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_build_confirmation_needed_is_false_for_a_no_tests_stall() {
        let dir = git_project_dir("nbc-needed-false-no-tests");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
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
        // No build action was ever planned for this task — a genuine
        // no-tests stall (or any other `needs_user_action` reason) must
        // never be mistaken for a build-insufficient stall.
        assert!(!no_build_confirmation_needed(&db, &project_id, &task_id).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_build_confirmation_needed_is_false_once_a_confirmation_resumed_the_task() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nbc-needed-false-resumed", "codex_cli");
        confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::Queued);
        assert!(!no_build_confirmation_needed(&db, &project_id, &task_id).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_build_confirmation_needed_is_true_again_for_a_half_success_state() {
        // The exact half-success window closed by the fourth re-review: a
        // confirmation row exists but the build action was never reset and
        // the task never left `needs_user_action` — the UI must still be
        // able to detect that a (retry) confirmation is what is needed.
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nbc-needed-true-half-success", "codex_cli");
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);
        assert!(no_build_confirmation_needed(&db, &project_id, &task_id).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_build_confirmation_needed_is_false_for_a_genuine_build_failure() {
        let dir = git_project_dir("nbc-needed-false-genuine-failure");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\"","build":"node -e \"process.exit(1)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(
            outcome.task.state,
            TaskState::Failed,
            "sanity check: a genuine, persistent build failure must exhaust the one fix pass"
        );
        assert!(!no_build_confirmation_needed(&db, &project_id, &task_id).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_build_confirmation_needed_rejects_a_project_id_that_does_not_own_the_task() {
        let (dir, db, _project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nbc-needed-wrong-project", "codex_cli");
        assert!(!no_build_confirmation_needed(&db, "some-other-project", &task_id).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_computes_round_1_automatically() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-round1-auto", "codex_cli");
        let confirmation =
            confirm_no_build_applicable(&db, &project_id, &task_id, Some("round 1 confirm"))
                .unwrap();
        assert_eq!(confirmation.round, 1);
        assert_eq!(confirmation.result_generation, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_computes_round_2_automatically() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_2_build_insufficient("cb-round2-auto", "codex_cli");
        let confirmation =
            confirm_no_build_applicable(&db, &project_id, &task_id, Some("round 2 confirm"))
                .unwrap();
        assert_eq!(
            confirmation.round, 2,
            "the backend must compute round 2 on its own — no caller ever supplies it"
        );
        assert_eq!(confirmation.result_generation, 0);
        // Round 1's own confirmation slot must remain empty — this call
        // was genuinely about round 2.
        let round1_confirmation = db
            .find_no_build_confirmation(&project_id, &task_id, 0, 1)
            .unwrap();
        assert!(round1_confirmation.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_rejects_a_task_from_a_different_project() {
        let (dir, db, _project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-wrong-project", "codex_cli");
        let result = confirm_no_build_applicable(&db, "some-other-project-id", &task_id, None);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_rejects_every_non_needs_user_action_state() {
        // Codex re-review remediation (final Major): exhaustively drives a
        // fresh task into each of Draft/Confirmed/Queued/Running/
        // SafeFixing/PreviewReady/AwaitingUserAcceptance/Completed/Failed/
        // Cancelled and asserts `confirm_no_build_applicable` rejects every
        // single one of them — only `needs_user_action` may ever accept a
        // no-build confirmation.
        let dir = git_project_dir("cb-reject-states");
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

        let assert_rejected = |db: &Database, task_id: &str, label: &str| {
            let result = confirm_no_build_applicable(db, &project.id, task_id, None);
            assert!(
                result.is_err(),
                "state {label} must reject a no-build confirmation, but it was accepted"
            );
        };

        // Draft: the freshest possible state, no transition at all.
        let draft_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "Draft".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        assert_rejected(&db, &draft_task.id, "Draft");

        // Confirmed.
        let confirmed_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "Confirmed".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        for state in [TaskState::AwaitingConfirmation, TaskState::Confirmed] {
            db.transition_task(
                &confirmed_task.id,
                state,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        }
        assert_rejected(&db, &confirmed_task.id, "Confirmed");

        // Queued.
        let queued_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "Queued".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        for state in [
            TaskState::AwaitingConfirmation,
            TaskState::Confirmed,
            TaskState::Queued,
        ] {
            db.transition_task(&queued_task.id, state, Actor::User, None, None, false, None)
                .unwrap();
        }
        assert_rejected(&db, &queued_task.id, "Queued");

        // Running.
        let running_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "Running".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        for state in [
            TaskState::AwaitingConfirmation,
            TaskState::Confirmed,
            TaskState::Queued,
            TaskState::Running,
        ] {
            db.transition_task(
                &running_task.id,
                state,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        }
        assert_rejected(&db, &running_task.id, "Running");

        // SafeFixing.
        let safe_fixing_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "SafeFixing".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
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
            TaskState::SafeFixing,
        ] {
            db.transition_task(
                &safe_fixing_task.id,
                state,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        }
        assert_rejected(&db, &safe_fixing_task.id, "SafeFixing");

        // PreviewReady.
        let preview_ready_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "PreviewReady".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
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
            TaskState::PreviewReady,
        ] {
            db.transition_task(
                &preview_ready_task.id,
                state,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        }
        assert_rejected(&db, &preview_ready_task.id, "PreviewReady");

        // AwaitingUserAcceptance.
        let awaiting_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "AwaitingUserAcceptance".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
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
            TaskState::PreviewReady,
            TaskState::AwaitingUserAcceptance,
        ] {
            db.transition_task(
                &awaiting_task.id,
                state,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        }
        assert_rejected(&db, &awaiting_task.id, "AwaitingUserAcceptance");

        // Completed (requires a real prior acceptance record).
        let completed_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "Completed".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
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
            TaskState::PreviewReady,
            TaskState::AwaitingUserAcceptance,
        ] {
            db.transition_task(
                &completed_task.id,
                state,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        }
        db.record_user_acceptance(&project.id, &completed_task.id, true, None)
            .unwrap();
        db.transition_task(
            &completed_task.id,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        assert_rejected(&db, &completed_task.id, "Completed");

        // Failed (direct edge from Running).
        let failed_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "Failed".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        for state in [
            TaskState::AwaitingConfirmation,
            TaskState::Confirmed,
            TaskState::Queued,
            TaskState::Running,
            TaskState::Failed,
        ] {
            db.transition_task(&failed_task.id, state, Actor::User, None, None, false, None)
                .unwrap();
        }
        assert_rejected(&db, &failed_task.id, "Failed");

        // Cancelled (direct edge from Draft).
        let cancelled_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "Cancelled".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
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
        assert_rejected(&db, &cancelled_task.id, "Cancelled");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_rejects_when_stalled_on_no_tests_instead_of_build() {
        // needs_user_action, but the real stall reason is a missing test
        // command confirmation, not build detection — there is no build
        // action for the pending round at all yet (the loop never even
        // reached the build step).
        let dir = git_project_dir("cb-reject-no-tests-reason");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(outcome.task.state, TaskState::NeedsUserAction);

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(
            result.is_err(),
            "a task stalled on missing test confirmation must never accept a no-build confirmation"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_rejects_when_build_genuinely_failed() {
        let dir = git_project_dir("cb-reject-real-build-failure");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\"","build":"node -e \"process.exit(1)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(
            outcome.task.state,
            TaskState::Failed,
            "sanity check: a genuine, persistent build failure must exhaust the one fix pass"
        );

        // `Failed` itself is already rejected by the state gate, but
        // additionally prove the *action-level* signal (real "failed"
        // build evidence, no insufficient-detection marker) would also
        // have been rejected had the task somehow still been sitting in
        // `needs_user_action` — this directly targets the "genuinely
        // failed build must never be confirmable" requirement rather than
        // only the state gate.
        let build_key = build_action_key(&task_id, 0, 1);
        let action = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(action.status, ActionStatus::Failed);
        assert_ne!(
            action.error_summary.as_deref(),
            Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER),
            "a genuine build failure must never carry the insufficient-detection marker"
        );
        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_rejects_when_build_timed_out() {
        let dir = git_project_dir("cb-reject-build-timeout");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let build_key = build_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "build_execution", &build_key)
            .unwrap();
        db.start_action(&task_id, &build_key, false).unwrap();
        db.finish_action(&task_id, &build_key, ActionStatus::Failed, None, None)
            .unwrap();
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            build_command_redacted: Some("cargo build"),
            status: "timed_out",
            exit_code: None,
            reason: Some("构建执行超时"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
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

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(
            result.is_err(),
            "a timed-out build (no insufficient-detection marker) must never be confirmable"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_rejects_when_build_was_cancelled() {
        let dir = git_project_dir("cb-reject-build-cancelled");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let build_key = build_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "build_execution", &build_key)
            .unwrap();
        db.start_action(&task_id, &build_key, false).unwrap();
        db.finish_action(
            &task_id,
            &build_key,
            ActionStatus::Cancelled,
            None,
            Some("user cancellation during build execution"),
        )
        .unwrap();
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            build_command_redacted: Some("cargo build"),
            status: "cancelled",
            exit_code: None,
            reason: Some("用户在构建阶段取消了任务"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        // A cancelled build normally lands the whole *task* in
        // `Cancelled` (already covered by the exhaustive state-rejection
        // test); this directly targets the action-level signal too, in
        // case the task were ever somehow left in `needs_user_action`.
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

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_rejects_when_build_command_was_allowlist_rejected() {
        // A build command *was* detected (not `Insufficient`) but failed
        // the strict allowlist — this must never be confirmable via
        // "no build needed" either, even though it also produces
        // `insufficient_evidence` build_evidence; the distinguishing
        // signal is the action's own error_summary marker, not the shared
        // evidence status string.
        let dir = git_project_dir("cb-reject-allowlist-rejected");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let build_key = build_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "build_execution", &build_key)
            .unwrap();
        db.start_action(&task_id, &build_key, false).unwrap();
        db.finish_action(
            &task_id,
            &build_key,
            ActionStatus::Failed,
            None,
            Some("build command not allowlisted"),
        )
        .unwrap();
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            build_command_redacted: Some("cargo build --release"),
            status: "insufficient_evidence",
            exit_code: None,
            reason: Some("构建命令未通过安全允许列表校验，已拒绝执行"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
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

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(
            result.is_err(),
            "an allowlist-rejected build command must never be confirmable as 'no build needed'"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_is_idempotent_for_repeated_calls() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-idempotent-repeat", "codex_cli");
        let first =
            confirm_no_build_applicable(&db, &project_id, &task_id, Some("first call")).unwrap();
        let task_after_first = db.task(&task_id).unwrap();
        assert_eq!(task_after_first.state, TaskState::Queued);

        let second = confirm_no_build_applicable(
            &db,
            &project_id,
            &task_id,
            Some("second call, different text"),
        )
        .unwrap();
        assert_eq!(
            first.id, second.id,
            "a repeated confirmation must return the exact same existing row"
        );
        assert_eq!(
            first.reason, second.reason,
            "the second call's different reason text must never overwrite the first"
        );

        let all_confirmations_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM no_build_confirmations WHERE task_id = ?1",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            all_confirmations_count, 1,
            "a repeated confirmation must never create a second row"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_two_concurrent_requests_produce_at_most_one_row() {
        // Real concurrency: two threads, each opening its own connection to
        // the same on-disk database file, racing to confirm the same
        // (task_id, result_generation, round).
        let dir = temp_dir("cb-concurrent");
        let db_path = dir.join("nova.sqlite3");
        let db1 = Database::open(&db_path).unwrap();
        db1.migrate().unwrap();
        let project = db1
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
        let task = db1
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "T".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
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
            db1.transition_task(&task.id, state, Actor::User, None, None, false, None)
                .unwrap();
        }
        let build_key = build_action_key(&task.id, 0, 1);
        db1.plan_action(&task.id, "build_execution", &build_key)
            .unwrap();
        db1.start_action(&task.id, &build_key, false).unwrap();
        db1.finish_action(
            &task.id,
            &build_key,
            ActionStatus::Failed,
            None,
            Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER),
        )
        .unwrap();
        db1.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project.id,
            task_id: &task.id,
            result_generation: 0,
            round: 1,
            build_command_redacted: None,
            status: "insufficient_evidence",
            exit_code: None,
            reason: Some("未识别到已知的项目类型"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        db1.transition_task(
            &task.id,
            TaskState::NeedsUserAction,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let project_id_a = project.id.clone();
        let task_id_a = task.id.clone();
        let db_path_a = db_path.clone();
        let handle_a = thread::spawn(move || {
            let db = Database::open(&db_path_a).unwrap();
            confirm_no_build_applicable(&db, &project_id_a, &task_id_a, Some("from thread A"))
        });
        let project_id_b = project.id.clone();
        let task_id_b = task.id.clone();
        let db_path_b = db_path.clone();
        let handle_b = thread::spawn(move || {
            let db = Database::open(&db_path_b).unwrap();
            confirm_no_build_applicable(&db, &project_id_b, &task_id_b, Some("from thread B"))
        });
        let result_a = handle_a.join().unwrap();
        let result_b = handle_b.join().unwrap();

        // Both requests must observe success (never a raw UNIQUE
        // constraint error surfacing to either caller) and must agree on
        // exactly the same confirmation row.
        let confirmation_a = result_a.expect("thread A must not see a UNIQUE constraint error");
        let confirmation_b = result_b.expect("thread B must not see a UNIQUE constraint error");
        assert_eq!(confirmation_a.id, confirmation_b.id);

        let count: i64 = db1
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM no_build_confirmations WHERE task_id = ?1",
                rusqlite::params![task.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "two concurrent confirmations for the same key must produce at most one row"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_does_not_reset_a_successful_test_action() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-preserve-test-action", "codex_cli");
        let test_key = test_action_key(&task_id, 0, 1);
        let test_action_before = db.find_action(&task_id, &test_key).unwrap().unwrap();
        assert_eq!(test_action_before.status, ActionStatus::Succeeded);

        confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();

        let test_action_after = db.find_action(&task_id, &test_key).unwrap().unwrap();
        assert_eq!(
            test_action_after.status,
            ActionStatus::Succeeded,
            "confirming no-build must never reset an already-successful test action"
        );
        assert_eq!(
            test_action_before.attempt_number,
            test_action_after.attempt_number
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_does_not_reset_the_dev_action() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-preserve-dev-action", "codex_cli");
        let dev_key = format!("dev-{task_id}-round-1");
        let dev_action_before = db.find_action(&task_id, &dev_key).unwrap().unwrap();
        assert_eq!(dev_action_before.status, ActionStatus::Succeeded);

        confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();

        let dev_action_after = db.find_action(&task_id, &dev_key).unwrap().unwrap();
        assert_eq!(dev_action_after.status, ActionStatus::Succeeded);
        assert_eq!(
            dev_action_before.attempt_number,
            dev_action_after.attempt_number
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_for_round_2_never_touches_round_1_action_or_evidence() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_2_build_insufficient("cb-round2-preserves-round1", "codex_cli");
        let round1_build_key = build_action_key(&task_id, 0, 1);
        let round1_action_before = db
            .find_action(&task_id, &round1_build_key)
            .unwrap()
            .unwrap();
        let round1_evidence_before = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();

        let confirmation = confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();
        assert_eq!(confirmation.round, 2);

        let round1_action_after = db
            .find_action(&task_id, &round1_build_key)
            .unwrap()
            .unwrap();
        assert_eq!(round1_action_after.status, ActionStatus::Succeeded);
        assert_eq!(
            round1_action_before.attempt_number, round1_action_after.attempt_number,
            "round 2's confirmation must never touch round 1's build action"
        );
        let round1_evidence_after = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(round1_evidence_before.len(), round1_evidence_after.len());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_does_not_cross_into_a_different_generation() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-generation-scoped", "codex_cli");
        confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();

        let generation_1_confirmation = db
            .find_no_build_confirmation(&project_id, &task_id, 1, 1)
            .unwrap();
        assert!(
            generation_1_confirmation.is_none(),
            "a generation-0 confirmation must never silently cover generation 1 of the same task"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirming_no_build_applicable_and_resuming_never_triggers_a_third_development_call() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-no-third-dev-call", "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        confirm_no_build_applicable(&db, &project_id, &task_id, Some("no build needed")).unwrap();

        let resumed = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(resumed.task.state, TaskState::AwaitingUserAcceptance);
        // This `adapter` instance is only ever passed to the *resumed*
        // `run_core_loop` call — round 1's original development already
        // happened (and succeeded) against a *different* adapter instance
        // inside `stall_task_at_round_1_build_insufficient`, before the
        // build stall was ever reached. Zero calls on *this* instance is
        // therefore the strongest possible proof that resuming after the
        // no-build confirmation never re-invokes development at all —
        // never mind a third time.
        assert_eq!(
            adapter.development_call_count(),
            0,
            "resuming after a no-build confirmation must never repeat development — the dev \
             action was already succeeded before the stall even happened"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirming_no_build_applicable_and_resuming_only_continues_build_verification() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-only-continues-build", "codex_cli");
        let test_evidence_before = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(
            test_evidence_before.len(),
            1,
            "round 1's test step must already have produced its one real evidence row before \
             the build stall"
        );

        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();
        run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        let test_evidence_after = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(
            test_evidence_after.len(),
            1,
            "resuming after a no-build confirmation must never re-run the already-succeeded \
             test step — the test evidence row count must stay exactly the same"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_reason_is_redacted() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-reason-redacted", "codex_cli");
        let confirmation = confirm_no_build_applicable(
            &db,
            &project_id,
            &task_id,
            Some("api_key=sk-1234567890abcdef1234567890abcdef please skip build"),
        )
        .unwrap();
        assert!(!confirmation
            .reason
            .as_deref()
            .unwrap()
            .contains("sk-1234567890abcdef1234567890abcdef"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_reason_is_capped_at_a_safe_length() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-reason-length-cap", "codex_cli");
        let very_long_reason = "a".repeat(5000);
        let confirmation =
            confirm_no_build_applicable(&db, &project_id, &task_id, Some(&very_long_reason))
                .unwrap();
        assert!(
            confirmation.reason.as_deref().unwrap().len() < 5000,
            "an oversized reason must be capped, not stored in full"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_empty_reason_uses_a_safe_default() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("cb-reason-empty-default", "codex_cli");
        let confirmation =
            confirm_no_build_applicable(&db, &project_id, &task_id, Some("   ")).unwrap();
        assert!(
            confirmation
                .reason
                .as_deref()
                .is_some_and(|r| !r.trim().is_empty()),
            "an empty/whitespace-only reason must be replaced with a safe default explanation"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Codex re-review remediation (fourth re-review, the final Major):
    // half-success recovery for `reconcile_no_build_confirmation`. Every
    // test below constructs a confirmation row directly via
    // `db.record_no_build_confirmation` (never through
    // `confirm_no_build_applicable`) to stand in for a prior call that
    // wrote the confirmation and then crashed before finishing the
    // reset/transition steps — exactly the half-success window the fourth
    // re-review's Major identified. ---------------------------------------

    #[test]
    fn confirm_no_build_applicable_recovers_when_confirmation_written_but_reset_and_transition_never_ran(
    ) {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-case-b-full-repair", "codex_cli");
        let build_key = build_action_key(&task_id, 0, 1);
        let seeded = db
            .record_no_build_confirmation(&project_id, &task_id, 0, 1, Some("half-success seed"))
            .unwrap();
        assert_eq!(
            db.find_action(&task_id, &build_key)
                .unwrap()
                .unwrap()
                .status,
            ActionStatus::Failed
        );
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);

        let recovered =
            confirm_no_build_applicable(&db, &project_id, &task_id, Some("retry")).unwrap();
        assert_eq!(
            recovered.id, seeded.id,
            "recovery must return the exact same confirmation row that already existed, never a new one"
        );
        assert_eq!(
            db.find_action(&task_id, &build_key)
                .unwrap()
                .unwrap()
                .status,
            ActionStatus::Planned,
            "the build action must be reset even though the confirmation already existed"
        );
        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::Queued,
            "the task must be transitioned even though the confirmation already existed"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_recovers_when_reset_done_but_transition_never_ran() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-case-c-transition-only", "codex_cli");
        let build_key = build_action_key(&task_id, 0, 1);
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, Some("half-success seed"))
            .unwrap();
        db.reset_failed_action_for_retry(&task_id, &build_key)
            .unwrap();
        let action_before = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(action_before.status, ActionStatus::Planned);
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);

        confirm_no_build_applicable(&db, &project_id, &task_id, Some("retry")).unwrap();

        let action_after = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(
            action_after.attempt_number, action_before.attempt_number,
            "an already-reset action must never be reset a second time"
        );
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::Queued);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_is_a_safe_no_op_when_reset_and_transition_already_done() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-case-d-full-noop", "codex_cli");
        let build_key = build_action_key(&task_id, 0, 1);
        let seeded = db
            .record_no_build_confirmation(&project_id, &task_id, 0, 1, Some("already complete"))
            .unwrap();
        db.reset_failed_action_for_retry(&task_id, &build_key)
            .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let action_before = db.find_action(&task_id, &build_key).unwrap().unwrap();

        let result =
            confirm_no_build_applicable(&db, &project_id, &task_id, Some("retry")).unwrap();
        assert_eq!(result.id, seeded.id);

        let action_after = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(action_after.attempt_number, action_before.attempt_number);
        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::Queued,
            "a fully-complete operation must never be rolled back or repeated"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_does_not_roll_back_a_task_already_running() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-case-e-running", "codex_cli");
        let build_key = build_action_key(&task_id, 0, 1);
        let seeded = db
            .record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        db.reset_failed_action_for_retry(&task_id, &build_key)
            .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let action_before = db.find_action(&task_id, &build_key).unwrap().unwrap();

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();
        assert_eq!(result.id, seeded.id);

        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::Running,
            "a duplicate request must never roll a running task back"
        );
        let action_after = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(action_after.attempt_number, action_before.attempt_number);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_does_not_roll_back_a_task_already_preview_ready() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-case-e-preview-ready", "codex_cli");
        let build_key = build_action_key(&task_id, 0, 1);
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        db.reset_failed_action_for_retry(&task_id, &build_key)
            .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::PreviewReady,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let action_before = db.find_action(&task_id, &build_key).unwrap().unwrap();
        let confirmation_count_before: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM no_build_confirmations WHERE task_id = ?1",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();

        // A confirmation legitimately tied to generation 0 no longer
        // applies once `preview_ready` bumps the task into generation 1 —
        // a duplicate/late request at this point correctly finds nothing
        // to reuse for the new generation and refuses to fabricate a fresh
        // confirmation against a task that is no longer
        // `needs_user_action`, rather than silently pretending success.
        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(
            result.is_err(),
            "a stale duplicate request must fail closed once the task has moved to a new \
             generation, never silently succeed"
        );

        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::PreviewReady,
            "must never roll back an already-preview-ready task"
        );
        let action_after = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(action_after.attempt_number, action_before.attempt_number);
        assert_eq!(action_after.status, action_before.status);
        let confirmation_count_after: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM no_build_confirmations WHERE task_id = ?1",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            confirmation_count_before, confirmation_count_after,
            "must never fabricate a new confirmation row either"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_does_not_roll_back_a_completed_task() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-case-e-completed", "codex_cli");
        let build_key = build_action_key(&task_id, 0, 1);
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        db.reset_failed_action_for_retry(&task_id, &build_key)
            .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::PreviewReady,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::AwaitingUserAcceptance,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.record_user_acceptance(&project_id, &task_id, true, None)
            .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let action_before = db.find_action(&task_id, &build_key).unwrap().unwrap();

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(
            result.is_err(),
            "must never allow re-confirmation once a task is completed"
        );

        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::Completed,
            "must never roll back a completed task"
        );
        let action_after = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(action_after.attempt_number, action_before.attempt_number);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_repairs_a_reset_that_never_happened() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-repair-reset", "codex_cli");
        let build_key = build_action_key(&task_id, 0, 1);
        db.record_no_build_confirmation(
            &project_id,
            &task_id,
            0,
            1,
            Some("crashed right after this write"),
        )
        .unwrap();
        let action_before = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(action_before.status, ActionStatus::Failed);
        assert_eq!(
            action_before.error_summary.as_deref(),
            Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER)
        );

        confirm_no_build_applicable(&db, &project_id, &task_id, Some("retry after crash")).unwrap();

        let action_after = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(
            action_after.status,
            ActionStatus::Planned,
            "a retry must repair a reset that never ran the first time"
        );
        assert_eq!(
            action_after.attempt_number,
            action_before.attempt_number + 1,
            "the repair must go through the real reset path, bumping attempt_number, never a \
             raw status overwrite"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_repairs_a_transition_that_never_happened() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-repair-transition", "codex_cli");
        let build_key = build_action_key(&task_id, 0, 1);
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, Some("crashed after reset"))
            .unwrap();
        db.reset_failed_action_for_retry(&task_id, &build_key)
            .unwrap();
        // The task's original run through `stall_task_at_round_1_build_
        // insufficient` already recorded one `-> queued` event of its own
        // (`confirmed -> queued`, before it ever reached the build stall)
        // — the sanity check below must compare against that baseline
        // count, not assume zero.
        let events_before: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM task_state_events WHERE task_id = ?1 AND new_state = 'queued'",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::NeedsUserAction,
            "sanity check: the repair transition genuinely never happened yet"
        );

        confirm_no_build_applicable(&db, &project_id, &task_id, Some("retry after crash")).unwrap();

        assert_eq!(db.task(&task_id).unwrap().state, TaskState::Queued);
        let events_after: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM task_state_events WHERE task_id = ?1 AND new_state = 'queued'",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            events_after,
            events_before + 1,
            "the repair must go through the real state machine (transition_task), leaving a \
             genuine new audit event, never a raw UPDATE"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_retry_never_inserts_a_second_confirmation_row() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-no-duplicate-insert", "codex_cli");
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, Some("seed"))
            .unwrap();

        for _ in 0..3 {
            confirm_no_build_applicable(&db, &project_id, &task_id, Some("retry")).unwrap();
        }

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM no_build_confirmations WHERE task_id = ?1",
                rusqlite::params![task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "repeated recovery retries must never create a second confirmation row"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_retry_never_resets_the_test_action() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-no-reset-test-action", "codex_cli");
        let test_key = test_action_key(&task_id, 0, 1);
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let test_action_before = db.find_action(&task_id, &test_key).unwrap().unwrap();
        assert_eq!(test_action_before.status, ActionStatus::Succeeded);

        confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();

        let test_action_after = db.find_action(&task_id, &test_key).unwrap().unwrap();
        assert_eq!(test_action_after.status, ActionStatus::Succeeded);
        assert_eq!(
            test_action_after.attempt_number, test_action_before.attempt_number,
            "recovering a half-success build confirmation must never touch the already-succeeded \
             test action"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_retry_never_resets_the_dev_action() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-no-reset-dev-action", "codex_cli");
        let dev_key = format!("dev-{task_id}-round-1");
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let dev_action_before = db.find_action(&task_id, &dev_key).unwrap().unwrap();
        assert_eq!(dev_action_before.status, ActionStatus::Succeeded);

        confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();

        let dev_action_after = db.find_action(&task_id, &dev_key).unwrap().unwrap();
        assert_eq!(dev_action_after.status, ActionStatus::Succeeded);
        assert_eq!(
            dev_action_after.attempt_number, dev_action_before.attempt_number,
            "recovering a half-success build confirmation must never touch the already-succeeded \
             dev action"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_retry_and_resume_never_triggers_a_third_development_call() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-no-third-dev-call-recovery", "codex_cli");
        db.record_no_build_confirmation(
            &project_id,
            &task_id,
            0,
            1,
            Some("crashed before reset/transition"),
        )
        .unwrap();

        confirm_no_build_applicable(&db, &project_id, &task_id, Some("retry")).unwrap();

        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        let resumed = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(resumed.task.state, TaskState::AwaitingUserAcceptance);
        assert_eq!(
            adapter.development_call_count(),
            0,
            "recovering a half-success confirmation and resuming must never repeat development"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_retry_and_resume_never_reruns_an_already_successful_test() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-no-rerun-test-recovery", "codex_cli");
        let test_evidence_before = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(test_evidence_before.len(), 1);
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();

        confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        let test_evidence_after = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(
            test_evidence_after.len(),
            1,
            "recovering and resuming must never re-run the already-succeeded test step"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_retry_never_reruns_an_already_successful_round_1_build() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_2_build_insufficient("nr-no-rerun-round1-build", "codex_cli");
        let round1_build_key = build_action_key(&task_id, 0, 1);
        let round1_action_before = db
            .find_action(&task_id, &round1_build_key)
            .unwrap()
            .unwrap();
        let round1_evidence_before = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        db.record_no_build_confirmation(&project_id, &task_id, 0, 2, None)
            .unwrap();

        confirm_no_build_applicable(&db, &project_id, &task_id, None).unwrap();

        let round1_action_after = db
            .find_action(&task_id, &round1_build_key)
            .unwrap()
            .unwrap();
        assert_eq!(
            round1_action_after.attempt_number, round1_action_before.attempt_number,
            "recovering round 2's confirmation must never touch round 1's already-succeeded \
             build action"
        );
        let round1_evidence_after = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(round1_evidence_before.len(), round1_evidence_after.len());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_fails_closed_on_a_generation_mismatch() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-wrong-generation", "codex_cli");
        // A confirmation genuinely exists for the task's real generation
        // (0)/round (1) combination...
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        // ...but the task's own `result_generation` is then externally
        // corrupted to a value that does not match where the real stalled
        // build action and confirmation actually live — simulating a data
        // inconsistency, never something `reconcile_no_build_confirmation`
        // itself could produce.
        db.conn()
            .execute(
                "UPDATE tasks SET result_generation = 3 WHERE id = ?1",
                rusqlite::params![task_id],
            )
            .unwrap();

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(
            result.is_err(),
            "a generation that does not match any real stalled build action must fail closed, \
             never fabricate a confirmation"
        );

        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::NeedsUserAction,
            "a failed-closed rejection must never itself change task state"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_fails_closed_on_a_round_mismatch() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-wrong-round", "codex_cli");
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        // Seed a round-2 development action directly — this alone makes
        // `current_pending_round` compute round 2 on the very next call,
        // even though the real stalled build action and confirmation are
        // for round 1. A genuine round 2 would never exist without round
        // 1 having first succeeded (dev/test/build) or been confirmed
        // no-build — this row exists only to construct the mismatch.
        let round2_dev_key = format!("dev-{task_id}-round-2");
        db.plan_action(&task_id, "ai_development", &round2_dev_key)
            .unwrap();

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(
            result.is_err(),
            "a round that does not match any real stalled build action must fail closed"
        );

        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);
        let round1_build_key = build_action_key(&task_id, 0, 1);
        assert_eq!(
            db.find_action(&task_id, &round1_build_key)
                .unwrap()
                .unwrap()
                .status,
            ActionStatus::Failed,
            "round 1's real stalled action must be left untouched by a round-2-scoped rejection"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_fails_closed_when_the_failed_action_marker_does_not_match() {
        // Constructed directly at the Action Ledger level (not via a
        // genuine `run_build_for_round` stall): a build action that is
        // `failed` for some unrelated reason, alongside a confirmation row
        // that should never have been possible to create for it —
        // simulating external tampering or a bug elsewhere, never
        // something this function's own Case A gate could produce (that
        // gate already requires the exact marker before ever writing a
        // confirmation).
        let dir = git_project_dir("nr-wrong-marker");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let build_key = build_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "build_execution", &build_key)
            .unwrap();
        db.start_action(&task_id, &build_key, false).unwrap();
        db.finish_action(
            &task_id,
            &build_key,
            ActionStatus::Failed,
            None,
            Some("build command not allowlisted"),
        )
        .unwrap();
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
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(
            result.is_err(),
            "a failed action whose marker does not match must fail closed, never be silently \
             reset"
        );

        let action_after = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(
            action_after.status,
            ActionStatus::Failed,
            "must never reset an action whose marker does not match the confirmation it \
             supposedly answers for"
        );
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_fails_closed_on_a_confirmation_project_mismatch() {
        let (dir, db, project_id, task_id) =
            stall_task_at_round_1_build_insufficient("nr-wrong-project", "codex_cli");
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let other_dir = temp_dir("nr-wrong-project-other");
        let other_project = db
            .create_project(&CreateProjectRequest {
                name: "Other".to_string(),
                local_path: other_dir.to_string_lossy().into_owned(),
                source_type: "local".to_string(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap();

        let result = confirm_no_build_applicable(&db, &other_project.id, &task_id, None);
        assert!(
            result.is_err(),
            "a project_id that does not own the task must be rejected even during recovery, \
             never silently reconciled"
        );

        let build_key = build_action_key(&task_id, 0, 1);
        assert_eq!(
            db.find_action(&task_id, &build_key)
                .unwrap()
                .unwrap()
                .status,
            ActionStatus::Failed,
            "a rejected cross-project request must never touch the real task's build action"
        );
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&other_dir);
    }

    #[test]
    fn confirm_no_build_applicable_two_concurrent_requests_converge_to_the_same_complete_final_state(
    ) {
        // Beyond `confirm_no_build_applicable_two_concurrent_requests_
        // produce_at_most_one_row` (which only proves row-count safety):
        // both racing requests must converge on the same *complete*
        // business state — the task actually unblocked and the build
        // action actually reset — not merely on a single confirmation row.
        let dir = temp_dir("nr-concurrent-converge");
        let db_path = dir.join("nova.sqlite3");
        let db1 = Database::open(&db_path).unwrap();
        db1.migrate().unwrap();
        let project = db1
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
        let task = db1
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "T".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
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
            db1.transition_task(&task.id, state, Actor::User, None, None, false, None)
                .unwrap();
        }
        let build_key = build_action_key(&task.id, 0, 1);
        db1.plan_action(&task.id, "build_execution", &build_key)
            .unwrap();
        db1.start_action(&task.id, &build_key, false).unwrap();
        db1.finish_action(
            &task.id,
            &build_key,
            ActionStatus::Failed,
            None,
            Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER),
        )
        .unwrap();
        db1.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project.id,
            task_id: &task.id,
            result_generation: 0,
            round: 1,
            build_command_redacted: None,
            status: "insufficient_evidence",
            exit_code: None,
            reason: Some("未识别到已知的项目类型"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        db1.transition_task(
            &task.id,
            TaskState::NeedsUserAction,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let project_id_a = project.id.clone();
        let task_id_a = task.id.clone();
        let db_path_a = db_path.clone();
        let handle_a = thread::spawn(move || {
            let db = Database::open(&db_path_a).unwrap();
            confirm_no_build_applicable(&db, &project_id_a, &task_id_a, Some("from thread A"))
        });
        let project_id_b = project.id.clone();
        let task_id_b = task.id.clone();
        let db_path_b = db_path.clone();
        let handle_b = thread::spawn(move || {
            let db = Database::open(&db_path_b).unwrap();
            confirm_no_build_applicable(&db, &project_id_b, &task_id_b, Some("from thread B"))
        });
        let confirmation_a = handle_a.join().unwrap().expect("thread A must succeed");
        let confirmation_b = handle_b.join().unwrap().expect("thread B must succeed");
        assert_eq!(confirmation_a.id, confirmation_b.id);

        assert_eq!(
            db1.task(&task.id).unwrap().state,
            TaskState::Queued,
            "both concurrent requests must converge on the task actually being unblocked, not \
             just a confirmation row existing"
        );
        let action_after = db1.find_action(&task.id, &build_key).unwrap().unwrap();
        assert_eq!(
            action_after.status,
            ActionStatus::Planned,
            "both concurrent requests must converge on the build action actually being reset"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirm_no_build_applicable_can_reconcile_a_half_success_state_after_reopening_the_database()
    {
        let dir = temp_dir("nr-restart-recovery");
        let db_path = dir.join("nova.sqlite3");
        let db1 = Database::open(&db_path).unwrap();
        db1.migrate().unwrap();
        let project = db1
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
        let task = db1
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "T".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
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
            db1.transition_task(&task.id, state, Actor::User, None, None, false, None)
                .unwrap();
        }
        let build_key = build_action_key(&task.id, 0, 1);
        db1.plan_action(&task.id, "build_execution", &build_key)
            .unwrap();
        db1.start_action(&task.id, &build_key, false).unwrap();
        db1.finish_action(
            &task.id,
            &build_key,
            ActionStatus::Failed,
            None,
            Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER),
        )
        .unwrap();
        db1.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project.id,
            task_id: &task.id,
            result_generation: 0,
            round: 1,
            build_command_redacted: None,
            status: "insufficient_evidence",
            exit_code: None,
            reason: Some("未识别到已知的项目类型"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        db1.transition_task(
            &task.id,
            TaskState::NeedsUserAction,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        // The confirmation write "succeeded" (step 3) but the process is
        // simulated to have stopped here — the reset and transition never
        // ran, and the app is now "restarted": `db1` is dropped and a
        // brand new `Database` handle is opened against the same on-disk
        // file.
        let seeded = db1
            .record_no_build_confirmation(&project.id, &task.id, 0, 1, Some("before restart"))
            .unwrap();
        drop(db1);

        let db2 = Database::open(&db_path).unwrap();
        let recovered =
            confirm_no_build_applicable(&db2, &project.id, &task.id, Some("after restart"))
                .unwrap();
        assert_eq!(
            recovered.id, seeded.id,
            "recovery after a restart must return the same confirmation row that survived on disk"
        );
        assert_eq!(
            db2.task(&task.id).unwrap().state,
            TaskState::Queued,
            "recovery after a restart must still complete the reset and transition"
        );
        let action_after = db2.find_action(&task.id, &build_key).unwrap().unwrap();
        assert_eq!(action_after.status, ActionStatus::Planned);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- v0.1.0 scope correction (Work Package 2 remediation, Major 2):
    // stress coverage for `reconcile_no_build_confirmation`'s new
    // concurrent-convergence retry wrapper. ------------------------------

    /// Shared setup for the concurrency tests below — a real, file-backed
    /// database with a task genuinely stalled on build detection,
    /// identical to the fixture the two pre-existing concurrent tests
    /// above build inline, factored out so these new tests do not each
    /// duplicate the same ~70 lines.
    fn file_backed_build_insufficient_stall(
        name: &str,
    ) -> (PathBuf, PathBuf, String, String, String) {
        let dir = temp_dir(name);
        let db_path = dir.join("nova.sqlite3");
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
                    ai_kind: Some("codex_cli".to_string()),
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
            db.transition_task(&task.id, state, Actor::User, None, None, false, None)
                .unwrap();
        }
        let build_key = build_action_key(&task.id, 0, 1);
        db.plan_action(&task.id, "build_execution", &build_key)
            .unwrap();
        db.start_action(&task.id, &build_key, false).unwrap();
        db.finish_action(
            &task.id,
            &build_key,
            ActionStatus::Failed,
            None,
            Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER),
        )
        .unwrap();
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project.id,
            task_id: &task.id,
            result_generation: 0,
            round: 1,
            build_command_redacted: None,
            status: "insufficient_evidence",
            exit_code: None,
            reason: Some("未识别到已知的项目类型"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        db.transition_task(
            &task.id,
            TaskState::NeedsUserAction,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        (dir, db_path, project.id, task.id, build_key)
    }

    #[test]
    fn confirm_no_build_applicable_barrier_synchronized_race_converges_across_many_iterations() {
        // Real OS-thread concurrency, but with both threads released from
        // a `Barrier` at the exact same instant on every iteration (rather
        // than merely `thread::spawn`ed, which leaves a wide, unpredictable
        // gap before either thread's first statement runs) — this reliably
        // re-creates the two race windows fixed in this round (the
        // first-request gate's own re-validation, and the `transition_
        // task` CAS) on a large fraction of iterations, exercising the
        // insert/reset/transition interleavings without any sleep or
        // artificial serialization. 25 fresh, independent stalls; every
        // single one must fully converge.
        for i in 0..25 {
            let (dir, db_path, project_id, task_id, build_key) =
                file_backed_build_insufficient_stall(&format!("nr-barrier-race-{i}"));
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

            let barrier_a = barrier.clone();
            let project_id_a = project_id.clone();
            let task_id_a = task_id.clone();
            let db_path_a = db_path.clone();
            let handle_a = thread::spawn(move || {
                let db = Database::open(&db_path_a).unwrap();
                barrier_a.wait();
                confirm_no_build_applicable(&db, &project_id_a, &task_id_a, Some("A"))
            });
            let barrier_b = barrier.clone();
            let project_id_b = project_id.clone();
            let task_id_b = task_id.clone();
            let db_path_b = db_path.clone();
            let handle_b = thread::spawn(move || {
                let db = Database::open(&db_path_b).unwrap();
                barrier_b.wait();
                confirm_no_build_applicable(&db, &project_id_b, &task_id_b, Some("B"))
            });

            let result_a = handle_a.join().unwrap();
            let result_b = handle_b.join().unwrap();
            let confirmation_a = result_a.unwrap_or_else(|e| {
                panic!("iteration {i}: thread A must not surface a raw concurrency error: {e}")
            });
            let confirmation_b = result_b.unwrap_or_else(|e| {
                panic!("iteration {i}: thread B must not surface a raw concurrency error: {e}")
            });
            assert_eq!(
                confirmation_a.id, confirmation_b.id,
                "iteration {i}: both threads must converge on the same confirmation row"
            );

            let db_check = Database::open(&db_path).unwrap();
            let count: i64 = db_check
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM no_build_confirmations WHERE task_id = ?1",
                    rusqlite::params![task_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "iteration {i}: exactly one confirmation row");
            assert_eq!(
                db_check.task(&task_id).unwrap().state,
                TaskState::Queued,
                "iteration {i}: task must have converged to queued"
            );
            let action_after = db_check.find_action(&task_id, &build_key).unwrap().unwrap();
            assert_eq!(
                action_after.status,
                ActionStatus::Planned,
                "iteration {i}: build action must have converged to planned"
            );
            let _ = fs::remove_dir_all(&dir);
        }
    }

    // --- Work Package 3: additional concurrency/idempotency coverage -----

    #[test]
    fn two_concurrent_no_tests_confirmations_for_the_same_generation_round_never_duplicate_or_corrupt_state(
    ) {
        // Mirrors `confirm_no_tests_applicable`'s real two-step production
        // path (`Database::record_no_tests_confirmation` then
        // `unblock_test_execution_for_retry`) under genuine two-OS-thread,
        // barrier-synchronized concurrency, against a real file-backed
        // database. Unlike no-build's confirmation (which has a dedicated
        // bounded-retry convergence wrapper), no-tests confirmation relies
        // on the table's own `UNIQUE(task_id, result_generation, round)`
        // constraint as its sole concurrency backstop — this proves that
        // backstop is sufficient: never two rows, never a corrupted final
        // task/action state, and the losing caller's error (if any) is a
        // plain, contained `Err`, never a panic or a wrong task's data
        // getting touched.
        for i in 0..10 {
            let dir = git_project_dir(&format!("no-tests-concurrent-{i}"));
            let db_dir = temp_dir(&format!("no-tests-concurrent-{i}-db"));
            let db_path = db_dir.join("nova.sqlite3");
            let project_id;
            let task_id;
            {
                let db = Database::open(&db_path).unwrap();
                db.migrate().unwrap();
                let (pid, tid) = create_confirmed_task_in(&db, &dir, "codex_cli");
                project_id = pid;
                task_id = tid;
                let adapter =
                    MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
                let outcome = run_core_loop(
                    &db,
                    &task_id,
                    &adapter,
                    &CancellationToken::new(),
                    &test_app_data_dir(),
                )
                .unwrap();
                assert_eq!(
                    outcome.task.state,
                    TaskState::NeedsUserAction,
                    "iteration {i}: a bare project with no test command must stall for a real \
                     no-tests confirmation before this race begins"
                );
            }

            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let run = |label: &'static str| {
                let barrier = barrier.clone();
                let db_path = db_path.clone();
                let project_id = project_id.clone();
                let task_id = task_id.clone();
                thread::spawn(move || {
                    let db = Database::open(&db_path).unwrap();
                    let generation = db.task(&task_id).unwrap().result_generation;
                    barrier.wait();
                    let confirmation = db
                        .record_no_tests_confirmation(
                            &project_id,
                            &task_id,
                            generation,
                            1,
                            Some(label),
                        )
                        .ok();
                    // Only the winning caller (whose insert actually
                    // committed) calls the resume step — mirrors
                    // `confirm_no_tests_applicable`'s real sequencing,
                    // where the second call's `?` on the insert error
                    // would already have returned before ever reaching
                    // this line in the real Tauri command.
                    if confirmation.is_some() {
                        let _ = unblock_test_execution_for_retry(&db, &project_id, &task_id);
                    }
                    confirmation
                })
            };
            let handle_a = run("A");
            let handle_b = run("B");
            let result_a = handle_a.join().unwrap();
            let result_b = handle_b.join().unwrap();
            assert!(
                result_a.is_some() || result_b.is_some(),
                "iteration {i}: at least one of the two racing confirmations must succeed"
            );

            let db_check = Database::open(&db_path).unwrap();
            let count: i64 = db_check
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM no_tests_confirmations WHERE task_id = ?1",
                    rusqlite::params![task_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                count, 1,
                "iteration {i}: exactly one confirmation row must exist, regardless of which \
                 thread's insert won the race"
            );
            let final_task = db_check.task(&task_id).unwrap();
            assert_eq!(
                final_task.state,
                TaskState::Queued,
                "iteration {i}: the task must converge to queued exactly once, not error out or \
                 get stuck"
            );
            let _ = fs::remove_dir_all(&dir);
            let _ = fs::remove_dir_all(&db_dir);
        }
    }

    #[test]
    fn two_threads_racing_different_generations_test_actions_for_the_same_round_never_cross_contaminate(
    ) {
        // Real two-OS-thread, barrier-synchronized concurrency: one thread
        // drives generation 0's round 1 test action, the other drives a
        // *different* generation's (1) round 1 test action for the *same*
        // task, at the same instant. The generation-aware key
        // (`test_action_key`) must keep the two completely independent —
        // no shared idempotency key, no cross-generation evidence
        // adoption, no panic from a unique-constraint collision that
        // should never happen because the keys genuinely differ.
        let dir = git_project_dir("test-action-parallel-generations");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add passing test"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let db_dir = temp_dir("test-action-parallel-generations-db");
        let db_path = db_dir.join("nova.sqlite3");
        let project_id;
        let task_id;
        {
            let db = Database::open(&db_path).unwrap();
            db.migrate().unwrap();
            let (pid, tid) = create_confirmed_task_in(&db, &dir, "codex_cli");
            project_id = pid;
            task_id = tid;
        }

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let barrier_a = barrier.clone();
        let db_path_a = db_path.clone();
        let project_id_a = project_id.clone();
        let task_id_a = task_id.clone();
        let dir_a = dir.clone();
        let handle_a = thread::spawn(move || {
            let db = Database::open(&db_path_a).unwrap();
            barrier_a.wait();
            run_tests_for_round(
                &db,
                &project_id_a,
                &task_id_a,
                &dir_a,
                1,
                &CancellationToken::new(),
            )
        });
        let barrier_b = barrier.clone();
        let db_path_b = db_path.clone();
        let project_id_b = project_id.clone();
        let task_id_b = task_id.clone();
        let dir_b = dir.clone();
        let handle_b = thread::spawn(move || {
            let db = Database::open(&db_path_b).unwrap();
            // Simulate generation 1 already being under way for this same
            // task/round by planning its action directly — a real
            // production key, just for a generation this task has not
            // actually reached yet (the point is purely key isolation,
            // not a realistic generation-2 product flow).
            let key = test_action_key(&task_id_b, 1, 1);
            db.plan_action(&task_id_b, "test_execution", &key).unwrap();
            db.start_action(&task_id_b, &key, false).unwrap();
            barrier_b.wait();
            let outcome = test_runner::run_tests(
                "node",
                &["-e".to_string(), "process.exit(0)".to_string()],
                &dir_b,
                TEST_TIMEOUT,
                CancellationToken::new(),
            );
            db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
                project_id: &project_id_b,
                task_id: &task_id_b,
                result_generation: 1,
                round: 1,
                test_command_redacted: Some("node -e ..."),
                status: if outcome.process_succeeded {
                    "passed"
                } else {
                    "failed"
                },
                passed_count: outcome.passed.unwrap_or(0),
                failed_count: outcome.failed.unwrap_or(0),
                skipped_count: outcome.skipped.unwrap_or(0),
                exit_code: outcome.exit_code.map(i64::from),
                reason: None,
                stdout_ref: None,
                stderr_ref: None,
            })
            .unwrap();
            db.finish_action(&task_id_b, &key, ActionStatus::Succeeded, None, None)
                .unwrap();
        });

        let result_a = handle_a.join().unwrap().unwrap();
        handle_b.join().unwrap();

        assert!(
            matches!(result_a, TestRoundOutcome::Passed(_)),
            "generation 0's real test run must succeed on its own, unaffected by generation 1's \
             concurrent activity: {result_a:?}"
        );
        let db_check = Database::open(&db_path).unwrap();
        let gen0_evidence = db_check
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        let gen1_evidence = db_check
            .list_test_evidence_for_round(&project_id, &task_id, 1, 1)
            .unwrap();
        assert_eq!(gen0_evidence.len(), 1);
        assert_eq!(gen1_evidence.len(), 1);
        assert_ne!(
            gen0_evidence[0].id, gen1_evidence[0].id,
            "the two generations must never share or overwrite each other's evidence"
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[test]
    fn concurrent_retry_wrapper_still_fails_closed_for_a_genuine_marker_mismatch() {
        // The new retry wrapper must never turn a *genuine* inconsistency
        // into a false "success" merely because it retries a few times —
        // this reconstructs the same Case F/marker-mismatch scenario
        // `confirm_no_build_applicable_fails_closed_when_the_failed_
        // action_marker_does_not_match` already covers for a single call,
        // and re-asserts it holds under the new multi-attempt wrapper.
        let dir = git_project_dir("nr-retry-still-fails-closed");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let build_key = build_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "build_execution", &build_key)
            .unwrap();
        db.start_action(&task_id, &build_key, false).unwrap();
        db.finish_action(
            &task_id,
            &build_key,
            ActionStatus::Failed,
            None,
            Some("build command not allowlisted"),
        )
        .unwrap();
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
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();

        let result = confirm_no_build_applicable(&db, &project_id, &task_id, None);
        assert!(
            result.is_err(),
            "a genuine marker mismatch must still fail closed under the retry wrapper, never be \
             retried into a false success"
        );
        let action_after = db.find_action(&task_id, &build_key).unwrap().unwrap();
        assert_eq!(
            action_after.status,
            ActionStatus::Failed,
            "must never reset an action for a marker mismatch, even after retries"
        );
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_race_result_survives_closing_and_reopening_the_database() {
        // Combines this round's concurrency fix with the fourth
        // re-review's restart-recovery guarantee: run the real two-thread
        // race to completion, then open a brand new connection (every
        // connection either racing thread held is already dropped by the
        // time its thread finished) and confirm the converged state is
        // still exactly right.
        let (dir, db_path, project_id, task_id, build_key) =
            file_backed_build_insufficient_stall("nr-race-then-restart");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

        let barrier_a = barrier.clone();
        let project_id_a = project_id.clone();
        let task_id_a = task_id.clone();
        let db_path_a = db_path.clone();
        let handle_a = thread::spawn(move || {
            let db = Database::open(&db_path_a).unwrap();
            barrier_a.wait();
            confirm_no_build_applicable(&db, &project_id_a, &task_id_a, Some("A"))
        });
        let barrier_b = barrier.clone();
        let project_id_b = project_id.clone();
        let task_id_b = task_id.clone();
        let db_path_b = db_path.clone();
        let handle_b = thread::spawn(move || {
            let db = Database::open(&db_path_b).unwrap();
            barrier_b.wait();
            confirm_no_build_applicable(&db, &project_id_b, &task_id_b, Some("B"))
        });
        let confirmation_a = handle_a.join().unwrap().expect("thread A must succeed");
        let confirmation_b = handle_b.join().unwrap().expect("thread B must succeed");
        assert_eq!(confirmation_a.id, confirmation_b.id);

        let db_after_restart = Database::open(&db_path).unwrap();
        assert_eq!(
            db_after_restart.task(&task_id).unwrap().state,
            TaskState::Queued
        );
        let action_after = db_after_restart
            .find_action(&task_id, &build_key)
            .unwrap()
            .unwrap();
        assert_eq!(action_after.status, ActionStatus::Planned);
        let found = db_after_restart
            .find_no_build_confirmation(&project_id, &task_id, 0, 1)
            .unwrap()
            .unwrap();
        assert_eq!(found.id, confirmation_a.id);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Codex re-review remediation (Major 3): generation-aware build
    // action key -----------------------------------------------------------

    #[test]
    fn build_action_key_differs_across_generations_for_the_same_task_and_round() {
        let key_gen0 = build_action_key("task-1", 0, 1);
        let key_gen1 = build_action_key("task-1", 1, 1);
        assert_ne!(
            key_gen0, key_gen1,
            "a generation-1 build action must never collide with generation 0's key"
        );
    }

    #[test]
    fn generation_1_build_succeeded_never_lets_generation_2_pass_without_its_own_evidence() {
        // Directly proves Major 3's core fix: an `AlreadySucceeded` action
        // gate result for the *current* generation, with no matching
        // build_evidence row, must fail closed — never `NoBuildApplicable`.
        let dir = git_project_dir("build-gen-mismatch-fail-closed");
        // A real, readable package.json with no build script — generation
        // 0's own detection is a clean, positively-confirmed
        // `NotApplicable`, so this test's outcome is not entangled with
        // Major 2's Insufficient/confirmation path.
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","scripts":{"test":"echo ok"}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");

        // Simulate generation 1's build action already marked succeeded
        // (e.g. by a prior, now-stale code path or a hand-crafted DB
        // state), but with NO corresponding build_evidence row for
        // generation 1.
        let stale_key = build_action_key(&task_id, 1, 1);
        db.plan_action(&task_id, "build_execution", &stale_key)
            .unwrap();
        db.start_action(&task_id, &stale_key, false).unwrap();
        db.finish_action(&task_id, &stale_key, ActionStatus::Succeeded, None, None)
            .unwrap();
        // Deliberately no `record_build_evidence` call for generation 1.

        // The task's *actual* current result_generation is still 0 (no
        // `preview_ready` transition has ever happened) — so a call for
        // generation 0 must go through the normal `Started` path (a fresh
        // key), completely unaffected by generation 1's stale, evidence-less
        // succeeded action. This proves the two generations' keys never
        // collide or interfere with each other.
        let result = run_build_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(
            matches!(result, BuildRoundOutcome::NoBuildApplicable(_)),
            "generation 0's own real build detection must run unaffected by generation 1's \
             unrelated stale action"
        );
        let gen0_evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(
            gen0_evidence.len(),
            1,
            "generation 0 must have gotten its own real build evidence, not reused generation 1's"
        );
        let gen1_evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 1, 1)
            .unwrap();
        assert!(
            gen1_evidence.is_empty(),
            "generation 1's stale succeeded action must never have produced build evidence"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn already_succeeded_build_action_with_no_matching_evidence_fails_closed() {
        // Directly exercises the `AlreadySucceeded` + missing-evidence
        // branch: the action ledger says generation 0/round 1's build
        // succeeded, but no build_evidence row exists for that exact pair
        // — must record `insufficient_evidence`, transition to
        // `NeedsUserAction`, and never report `NoBuildApplicable`.
        let dir = git_project_dir("build-already-succeeded-no-evidence");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let key = build_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "build_execution", &key).unwrap();
        db.start_action(&task_id, &key, false).unwrap();
        db.finish_action(&task_id, &key, ActionStatus::Succeeded, None, None)
            .unwrap();

        let result = run_build_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();

        assert!(
            matches!(result, BuildRoundOutcome::ActionUncertain),
            "a succeeded action with no matching evidence must fail closed, not report \
             NoBuildApplicable"
        );
        let evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].status, "insufficient_evidence");
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_1_build_evidence_never_substitutes_for_round_2() {
        let dir = git_project_dir("build-round-1-not-round-2");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            build_command_redacted: None,
            status: "no_build_applicable",
            exit_code: None,
            reason: Some("round 1 result"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();

        let round2_evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 2)
            .unwrap();
        assert!(
            round2_evidence.is_empty(),
            "round 1's build evidence must never be visible when querying round 2"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn main_flow_approve_reaches_awaiting_user_acceptance() {
        let dir = git_project_dir("main-flow");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop_dual_ai_v0_2(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        assert_eq!(adapter.development_call_count(), 1);
        assert_eq!(adapter.review_call_count(), 1);

        let reviews = db.list_task_reviews(&project_id, &task_id).unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].verdict, "approve");
        assert_eq!(reviews[0].round, 1);

        // The full success path also composes correctly with Phase 2's
        // existing acceptance/completion machinery.
        db.record_user_acceptance(&project_id, &task_id, true, None)
            .unwrap();
        let completed = db
            .transition_task(
                &task_id,
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

    // --- Second re-review (Major 5): accept_task_result / resync_task_documents ---

    #[test]
    fn accepting_a_result_completes_the_task_and_final_result_reflects_it() {
        let dir = git_project_dir("accept-result-completes");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let app_data_dir = test_app_data_dir();
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();
        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);

        let task = accept_task_result(
            &db,
            &project_id,
            &task_id,
            true,
            Some("looks good"),
            &app_data_dir,
        )
        .unwrap();
        assert_eq!(task.state, TaskState::Completed);
        assert!(!task.document_sync_pending);

        let final_result_path = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id)
            .join("FINAL_RESULT.md");
        let content =
            fs::read_to_string(fs::canonicalize(&final_result_path).unwrap_or(final_result_path))
                .unwrap();
        assert!(
            content.contains("已完成（用户已验收）"),
            "FINAL_RESULT.md must reflect the real completed state, not stay on \"awaiting acceptance\": {content}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejecting_a_result_moves_to_needs_user_action_and_final_result_reflects_it() {
        let dir = git_project_dir("reject-result-needs-action");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let app_data_dir = test_app_data_dir();
        run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();

        let task = accept_task_result(
            &db,
            &project_id,
            &task_id,
            false,
            Some("not quite"),
            &app_data_dir,
        )
        .unwrap();
        assert_eq!(task.state, TaskState::NeedsUserAction);

        let final_result_path = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id)
            .join("FINAL_RESULT.md");
        let content =
            fs::read_to_string(fs::canonicalize(&final_result_path).unwrap_or(final_result_path))
                .unwrap();
        assert!(
            content.contains("需要用户处理"),
            "FINAL_RESULT.md must reflect the rejection, not the acceptance workflow: {content}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn accepting_the_same_result_twice_is_refused_the_second_time_not_double_completed() {
        let dir = git_project_dir("accept-twice");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let app_data_dir = test_app_data_dir();
        run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();

        accept_task_result(&db, &project_id, &task_id, true, None, &app_data_dir).unwrap();
        let second = accept_task_result(&db, &project_id, &task_id, true, None, &app_data_dir);
        assert!(
            second.is_err(),
            "a task no longer in awaiting_user_acceptance must refuse a second acceptance, not silently re-accept"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn accept_task_result_refuses_a_project_id_that_does_not_own_the_task() {
        let dir_a = git_project_dir("accept-wrong-project-a");
        let dir_b = git_project_dir("accept-wrong-project-b");
        let (db, project_a, task_a) = db_with_confirmed_task(&dir_a, "codex_cli");
        confirm_no_tests(&db, &project_a, &task_a, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let app_data_dir = test_app_data_dir();
        run_core_loop(
            &db,
            &task_a,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
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
        let result = accept_task_result(&db, &project_b.id, &task_a, true, None, &app_data_dir);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn resync_task_documents_clears_a_pending_flag_and_regenerates() {
        let dir = git_project_dir("resync-documents");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let app_data_dir = test_app_data_dir();
        run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();
        accept_task_result(&db, &project_id, &task_id, true, None, &app_data_dir).unwrap();

        // Simulate a prior failed sync (e.g. a transient docs-directory
        // write error) that left the flag set despite the DB state already
        // being correct — `resync_task_documents` must be able to clear it
        // without requiring another accept/reject decision.
        db.set_document_sync_pending(&task_id, true).unwrap();
        assert!(db.task(&task_id).unwrap().document_sync_pending);

        let task = resync_task_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        assert!(!task.document_sync_pending);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cannot_start_the_loop_from_an_illegal_state() {
        let dir = git_project_dir("illegal-start");
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
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        // Still `draft` — never confirmed.
        let adapter = MockAdapter::new(vec![], vec![]);
        let result = run_core_loop(
            &db,
            &task.id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        );
        assert!(result.is_err());
        assert_eq!(
            adapter.development_call_count(),
            0,
            "must not touch the developer AI for an illegal start"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_fix_pass_then_still_rejected_ends_in_failed() {
        let dir = git_project_dir("one-fix-then-fail");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "claude_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1, 2]);
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success, MockOutcome::Success],
            vec![
                ReviewVerdict::ChangesRequired,
                ReviewVerdict::ChangesRequired,
            ],
        );

        let outcome = run_core_loop_dual_ai_v0_2(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::Failed);
        assert_eq!(
            adapter.development_call_count(),
            2,
            "exactly one fix pass means exactly two development invocations"
        );
        assert_eq!(
            adapter.review_call_count(),
            2,
            "first review plus exactly one second review"
        );

        let reviews = db.list_task_reviews(&project_id, &task_id).unwrap();
        assert_eq!(reviews.len(), 2);
        assert_eq!(reviews[0].round, 1);
        assert_eq!(reviews[1].round, 2);
        assert!(reviews.iter().all(|r| r.verdict == "changes_required"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_round_approve_after_one_fix_pass_still_succeeds() {
        let dir = git_project_dir("fix-then-approve");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1, 2]);
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success, MockOutcome::Success],
            vec![ReviewVerdict::ChangesRequired, ReviewVerdict::Approve],
        );

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        assert_eq!(
            outcome.task.result_generation, 1,
            "exactly one accepted result was ever produced"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn developer_process_failure_ends_the_loop_as_failed_without_a_review() {
        let dir = git_project_dir("dev-failure");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Failure], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::Failed);
        assert_eq!(
            adapter.review_call_count(),
            0,
            "a failed developer run must never reach the reviewer"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_cancellation_before_start_is_recorded_as_cancelled_not_run() {
        let dir = git_project_dir("user-cancel-before-start");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome =
            run_core_loop(&db, &task_id, &adapter, &cancel, &test_app_data_dir()).unwrap();

        assert_eq!(outcome.task.state, TaskState::Cancelled);
        assert_eq!(
            adapter.development_call_count(),
            0,
            "a pre-cancelled loop must never invoke the developer AI"
        );
        assert_eq!(adapter.review_call_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dirty_worktree_blocks_the_loop_before_any_ai_is_invoked() {
        let dir = git_project_dir("dirty-worktree");
        fs::write(dir.join("uncommitted.txt"), "oops").unwrap();
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::NeedsUserAction);
        assert_eq!(adapter.development_call_count(), 0);
        assert_eq!(adapter.review_call_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reviewer_write_attempt_is_technically_detected_and_stops_the_loop() {
        let dir = git_project_dir("reviewer-writes");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let violating_dir = dir.clone();
        *adapter.review_side_effect.lock().unwrap() = Some(Box::new(move |_req| {
            let _ = fs::write(
                violating_dir.join("reviewer_should_not_write_this.txt"),
                "oops",
            );
        }));

        let outcome = run_core_loop_dual_ai_v0_2(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::NeedsUserAction);
        let reviews = db.list_task_reviews(&project_id, &task_id).unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].verdict, "integrity_violation");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reviewer_is_never_given_the_real_project_path_at_all() {
        // Blocker 1's actual wiring proof: the `ReviewRequest` the adapter
        // receives points at an isolated copy, never at the real project
        // directory — checked directly against the field the (real)
        // `RealCliAdapter` would use as the reviewer's `cwd`.
        let dir = git_project_dir("blocker1-no-real-path");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let captured_workspace_dir: Arc<std::sync::Mutex<Option<PathBuf>>> =
            Arc::new(std::sync::Mutex::new(None));
        let captured_workspace_dir_for_closure = Arc::clone(&captured_workspace_dir);
        *adapter.review_side_effect.lock().unwrap() = Some(Box::new(move |req| {
            *captured_workspace_dir_for_closure.lock().unwrap() =
                Some(req.isolated_workspace_dir.clone());
        }));

        let app_data_dir = test_app_data_dir();
        run_core_loop_dual_ai_v0_2(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();

        let captured = captured_workspace_dir.lock().unwrap().clone().unwrap();
        assert_ne!(
            captured, dir,
            "the reviewer's cwd must never equal the real project directory"
        );
        assert!(
            !captured.starts_with(&dir),
            "the reviewer's cwd must never be nested inside the real project directory either"
        );
        assert!(
            captured.starts_with(fs::canonicalize(&app_data_dir).unwrap()),
            "the reviewer's cwd must be under the app-owned data directory"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn crash_recovery_stops_for_user_decision_instead_of_re_invoking_the_developer() {
        let dir = git_project_dir("crash-recovery");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");

        // Simulate a crash exactly while the developer AI's action was
        // `started` for round 1 (the task itself never got past `running`
        // in this simulation, matching a real mid-flight crash).
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let dev_key = format!("dev-{task_id}-round-1");
        db.plan_action(&task_id, "ai_development", &dev_key)
            .unwrap();
        db.start_action(&task_id, &dev_key, false).unwrap();

        // Startup recovery, exactly as `lib.rs` runs it before any command
        // is dispatched: sweeps the action to `uncertain` and the task to
        // `interrupted`.
        db.run_startup_recovery().unwrap();
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::Interrupted);
        assert_eq!(
            db.find_action(&task_id, &dev_key).unwrap().unwrap().status,
            ActStatus::Uncertain
        );

        // The user chooses to continue -> interrupted -> queued is a legal
        // Phase 2 edge; the core loop must then refuse to blindly re-run
        // the developer AI for the same round.
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::User,
            Some("用户选择继续"),
            None,
            false,
            None,
        )
        .unwrap();

        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::NeedsUserAction);
        assert_eq!(
            adapter.development_call_count(),
            0,
            "an uncertain prior attempt must never be silently retried"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Work Package 3: recovery/restart matrix on a real file-backed DB -
    //
    // The tests above (and Phase 2's own recovery suite) already prove
    // `run_startup_recovery`'s core sweep against an in-memory database.
    // These specifically use a real on-disk SQLite file, closed and
    // reopened as a fresh `Database` connection (simulating a genuine
    // process restart, not just a fresh struct), and call the real
    // `Database::run_startup_recovery` entry point `lib.rs` itself invokes
    // before any command is dispatched.

    #[test]
    fn a_real_process_restart_recovers_a_crashed_test_action_and_resumes_to_success() {
        let dir = git_project_dir("restart-recover-test-action");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\"","build":"node -e \"process.exit(0)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add passing scripts"])
            .current_dir(&dir)
            .status()
            .unwrap();

        let db_dir = temp_dir("restart-recover-test-action-db");
        let db_path = db_dir.join("nova.sqlite3");
        let app_data_dir = db_dir.join("app-data");
        let task_id;
        let project_id;
        {
            let db = Database::open(&db_path).unwrap();
            db.migrate().unwrap();
            let (pid, tid) = create_confirmed_task_in(&db, &dir, "codex_cli");
            project_id = pid;
            task_id = tid;
            db.transition_task(
                &task_id,
                TaskState::Queued,
                Actor::System,
                None,
                None,
                false,
                None,
            )
            .unwrap();
            db.transition_task(
                &task_id,
                TaskState::Running,
                Actor::System,
                None,
                None,
                false,
                None,
            )
            .unwrap();
            db.transition_task(
                &task_id,
                TaskState::Verifying,
                Actor::System,
                None,
                None,
                false,
                None,
            )
            .unwrap();
            let dev_key = format!("dev-{task_id}-round-1");
            db.plan_action(&task_id, "ai_development", &dev_key)
                .unwrap();
            db.start_action(&task_id, &dev_key, false).unwrap();
            db.finish_action(&task_id, &dev_key, ActionStatus::Succeeded, None, None)
                .unwrap();
            // Simulate the crash: the test-execution action (generation-
            // aware key) was planned and started, but the process died
            // before it could ever finish — no test_evidence row exists.
            let test_key = test_action_key(&task_id, 0, 1);
            db.plan_action(&task_id, "test_execution", &test_key)
                .unwrap();
            db.start_action(&task_id, &test_key, false).unwrap();
        } // real connection closed here.

        let db = Database::open(&db_path).unwrap();
        // Note: `run_startup_recovery`'s returned `Option<RecoverySummary>`
        // is purely a UI convenience keyed off `last_active_project` (not
        // set by this fixture) — the actual data-repair sweep this test
        // cares about (`interrupt_all_in_flight_tasks` +
        // `mark_started_actions_uncertain`) runs unconditionally before
        // that, regardless of the return value.
        db.run_startup_recovery().unwrap();
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::Interrupted);
        let test_key = test_action_key(&task_id, 0, 1);
        assert_eq!(
            db.find_action(&task_id, &test_key).unwrap().unwrap().status,
            ActionStatus::Uncertain,
            "a started-but-never-finished test action must never be left looking like it is \
             still running"
        );

        // The user reviews and chooses to continue: acknowledge the
        // uncertain test action (never an automatic retry) and requeue.
        db.acknowledge_uncertain_action(&task_id, &test_key)
            .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();
        assert_eq!(
            outcome.task.state,
            TaskState::AwaitingUserAcceptance,
            "the resumed run must actually complete for real, not get stuck again"
        );
        assert_eq!(
            adapter.development_call_count(),
            0,
            "round 1's already-succeeded development step must never repeat after recovery"
        );
        let evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].status, "passed");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[test]
    fn a_real_process_restart_recovers_a_crashed_build_action_and_resumes_to_success() {
        let dir = git_project_dir("restart-recover-build-action");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\"","build":"node -e \"process.exit(0)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add passing scripts"])
            .current_dir(&dir)
            .status()
            .unwrap();

        let db_dir = temp_dir("restart-recover-build-action-db");
        let db_path = db_dir.join("nova.sqlite3");
        let app_data_dir = db_dir.join("app-data");
        let task_id;
        let project_id;
        {
            let db = Database::open(&db_path).unwrap();
            db.migrate().unwrap();
            let (pid, tid) = create_confirmed_task_in(&db, &dir, "codex_cli");
            project_id = pid;
            task_id = tid;
            db.transition_task(
                &task_id,
                TaskState::Queued,
                Actor::System,
                None,
                None,
                false,
                None,
            )
            .unwrap();
            db.transition_task(
                &task_id,
                TaskState::Running,
                Actor::System,
                None,
                None,
                false,
                None,
            )
            .unwrap();
            db.transition_task(
                &task_id,
                TaskState::Verifying,
                Actor::System,
                None,
                None,
                false,
                None,
            )
            .unwrap();
            let dev_key = format!("dev-{task_id}-round-1");
            db.plan_action(&task_id, "ai_development", &dev_key)
                .unwrap();
            db.start_action(&task_id, &dev_key, false).unwrap();
            db.finish_action(&task_id, &dev_key, ActionStatus::Succeeded, None, None)
                .unwrap();
            let test_key = test_action_key(&task_id, 0, 1);
            db.plan_action(&task_id, "test_execution", &test_key)
                .unwrap();
            db.start_action(&task_id, &test_key, false).unwrap();
            db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
                project_id: &project_id,
                task_id: &task_id,
                result_generation: 0,
                round: 1,
                test_command_redacted: Some("npm test"),
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
            db.finish_action(&task_id, &test_key, ActionStatus::Succeeded, None, None)
                .unwrap();
            // Crash exactly while the build action was in flight.
            let build_key = build_action_key(&task_id, 0, 1);
            db.plan_action(&task_id, "build_execution", &build_key)
                .unwrap();
            db.start_action(&task_id, &build_key, false).unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        db.run_startup_recovery().unwrap();
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::Interrupted);
        let build_key = build_action_key(&task_id, 0, 1);
        assert_eq!(
            db.find_action(&task_id, &build_key)
                .unwrap()
                .unwrap()
                .status,
            ActionStatus::Uncertain
        );
        db.acknowledge_uncertain_action(&task_id, &build_key)
            .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();
        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        assert_eq!(
            adapter.development_call_count(),
            0,
            "development must never repeat"
        );
        let test_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(
            test_evidence.len(),
            1,
            "the already-passed test evidence must never be duplicated by a repeated run"
        );
        let build_evidence = db
            .list_build_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(build_evidence.len(), 1);
        assert_eq!(build_evidence[0].status, "passed");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[test]
    fn a_real_process_restart_never_touches_preview_ready_awaiting_acceptance_or_needs_user_action()
    {
        // `run_startup_recovery`'s sweep only ever force-interrupts
        // `running`/`verifying`/`safe_fixing` — a task already resting at
        // `preview_ready`, `awaiting_user_acceptance`, or
        // `needs_user_action` must survive a real restart completely
        // untouched, not be reset or reinterpreted.
        for (state_name, target) in [
            ("preview_ready", TaskState::PreviewReady),
            (
                "awaiting_user_acceptance",
                TaskState::AwaitingUserAcceptance,
            ),
            ("needs_user_action", TaskState::NeedsUserAction),
        ] {
            let dir = git_project_dir(&format!("restart-untouched-{state_name}"));
            let db_dir = temp_dir(&format!("restart-untouched-{state_name}-db"));
            let db_path = db_dir.join("nova.sqlite3");
            let task_id;
            {
                let db = Database::open(&db_path).unwrap();
                db.migrate().unwrap();
                let (_project_id, tid) = create_confirmed_task_in(&db, &dir, "codex_cli");
                task_id = tid;
                db.transition_task(
                    &task_id,
                    TaskState::Queued,
                    Actor::System,
                    None,
                    None,
                    false,
                    None,
                )
                .unwrap();
                db.transition_task(
                    &task_id,
                    TaskState::Running,
                    Actor::System,
                    None,
                    None,
                    false,
                    None,
                )
                .unwrap();
                db.transition_task(
                    &task_id,
                    TaskState::Verifying,
                    Actor::System,
                    None,
                    None,
                    false,
                    None,
                )
                .unwrap();
                if target == TaskState::NeedsUserAction {
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
                } else {
                    db.transition_task(
                        &task_id,
                        TaskState::PreviewReady,
                        Actor::System,
                        None,
                        None,
                        false,
                        None,
                    )
                    .unwrap();
                    if target == TaskState::AwaitingUserAcceptance {
                        db.transition_task(
                            &task_id,
                            TaskState::AwaitingUserAcceptance,
                            Actor::User,
                            None,
                            None,
                            false,
                            None,
                        )
                        .unwrap();
                    }
                }
            }

            let db = Database::open(&db_path).unwrap();
            db.run_startup_recovery().unwrap();
            assert_eq!(
                db.task(&task_id).unwrap().state,
                target,
                "a real restart must never move a task resting in {state_name} anywhere else"
            );
            let _ = fs::remove_dir_all(&dir);
            let _ = fs::remove_dir_all(&db_dir);
        }
    }

    #[test]
    fn a_real_process_restart_recovers_a_pending_document_resync() {
        // Simulates `accept_task_result`'s own best-effort-marker branch:
        // the accept/reject decision already committed, but seven-document
        // regeneration failed and left `document_sync_pending` set. A real
        // restart must not lose this fact, and `resync_task_documents`
        // (the real production retry path) must be able to clear it and
        // regenerate the documents for real.
        let dir = git_project_dir("restart-recover-doc-resync");
        let db_dir = temp_dir("restart-recover-doc-resync-db");
        let db_path = db_dir.join("nova.sqlite3");
        let app_data_dir = db_dir.join("app-data");
        let task_id;
        let project_id;
        {
            let db = Database::open(&db_path).unwrap();
            db.migrate().unwrap();
            let (pid, tid) = create_confirmed_task_in(&db, &dir, "codex_cli");
            project_id = pid;
            task_id = tid;
            for to in [
                TaskState::Queued,
                TaskState::Running,
                TaskState::Verifying,
                TaskState::PreviewReady,
                TaskState::AwaitingUserAcceptance,
            ] {
                db.transition_task(&task_id, to, Actor::System, None, None, false, None)
                    .unwrap();
            }
            db.record_user_acceptance(&project_id, &task_id, true, None)
                .unwrap();
            db.transition_task(
                &task_id,
                TaskState::Completed,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
            db.set_document_sync_pending(&task_id, true).unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        db.run_startup_recovery().unwrap();
        let task = db.task(&task_id).unwrap();
        assert_eq!(
            task.state,
            TaskState::Completed,
            "a completed task must never be reinterpreted by the recovery sweep"
        );
        assert!(
            task.document_sync_pending,
            "the pending-resync fact itself must survive a real restart"
        );

        let resynced = resync_task_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        assert!(!resynced.document_sync_pending);
        let final_result_path = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id)
            .join("FINAL_RESULT.md");
        let content = fs::read_to_string(&final_result_path).unwrap();
        assert!(content.contains("已完成（用户已验收）"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[test]
    fn resuming_after_the_developer_step_already_succeeded_does_not_repeat_it() {
        let dir = git_project_dir("resume-after-dev-succeeded");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        // Pre-seed the ledger as if the developer step already completed
        // successfully in an earlier, uninterrupted attempt.
        let dev_key = format!("dev-{task_id}-round-1");
        db.plan_action(&task_id, "ai_development", &dev_key)
            .unwrap();
        db.start_action(&task_id, &dev_key, false).unwrap();
        db.finish_action(&task_id, &dev_key, ActionStatus::Succeeded, None, None)
            .unwrap();
        confirm_no_tests(&db, &_project_id, &task_id, 0, &[1]);

        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let outcome = run_one_round(
            &db,
            &task_id,
            &_project_id,
            &dir,
            ToolKind::CodexCli,
            ToolKind::ClaudeCli,
            &adapter,
            &CancellationToken::new(),
            1,
            None,
            &test_app_data_dir(),
        )
        .unwrap();

        assert!(matches!(outcome, RoundOutcome::Approved));
        assert_eq!(
            adapter.development_call_count(),
            0,
            "the already-succeeded development step must not run again"
        );
        assert_eq!(
            adapter.review_call_count(),
            1,
            "the review step still must run"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_independent_connections_racing_the_same_task_only_one_runs_development() {
        let dir = git_project_dir("concurrent-core-loop");
        let harness_dir = temp_dir("concurrent-core-loop-db");
        let db_path = harness_dir.join("nova.sqlite3");

        let setup = Database::open(&db_path).unwrap();
        setup.migrate().unwrap();
        let project = setup
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
        let task = setup
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "T".to_string(),
                    goal: Some("goal".to_string()),
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        setup
            .transition_task(
                &task.id,
                TaskState::AwaitingConfirmation,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        setup
            .transition_task(
                &task.id,
                TaskState::Confirmed,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        drop(setup);

        let adapter = Arc::new(MockAdapter::new(
            vec![MockOutcome::Success],
            vec![ReviewVerdict::Approve],
        ));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let task_id = task.id.clone();
        let shared_app_data_dir = test_app_data_dir();

        let handles: Vec<_> = [(); 2]
            .into_iter()
            .map(|_| {
                let path = db_path.clone();
                let adapter = Arc::clone(&adapter);
                let barrier = Arc::clone(&barrier);
                let task_id = task_id.clone();
                let app_data_dir = shared_app_data_dir.clone();
                std::thread::spawn(move || {
                    let db = Database::open(&path).unwrap();
                    barrier.wait();
                    run_core_loop(
                        &db,
                        &task_id,
                        adapter.as_ref(),
                        &CancellationToken::new(),
                        &app_data_dir,
                    )
                })
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        // Both calls may return Ok (one genuinely drives the loop, the
        // other may observe an already-in-progress/finished state and
        // simply stop), but development must only ever really run once.
        for result in &results {
            assert!(result.is_ok() || result.is_err());
        }
        assert_eq!(
            adapter.development_call_count(),
            1,
            "two racing connections must never both invoke the developer AI for the same round"
        );

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&harness_dir);
    }

    #[test]
    fn evidence_and_reviews_stay_isolated_between_two_projects() {
        let dir_a = git_project_dir("isolation-a");
        let dir_b = git_project_dir("isolation-b");
        let (db, project_a, task_a) = db_with_confirmed_task(&dir_a, "codex_cli");
        let project_b_row = db
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
        let task_b_row = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project_b_row.id.clone(),
                    title: "B task".to_string(),
                    goal: Some("秘密-B".to_string()),
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("claude_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        db.transition_task(
            &task_b_row.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_b_row.id,
            TaskState::Confirmed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        confirm_no_tests(&db, &project_a, &task_a, 0, &[1]);
        confirm_no_tests(&db, &project_b_row.id, &task_b_row.id, 0, &[1, 2]);

        let adapter_a = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        run_core_loop_dual_ai_v0_2(
            &db,
            &task_a,
            &adapter_a,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        let adapter_b = MockAdapter::new(
            vec![MockOutcome::Success],
            vec![ReviewVerdict::ChangesRequired],
        );
        run_core_loop_dual_ai_v0_2(
            &db,
            &task_b_row.id,
            &adapter_b,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        let reviews_a = db.list_task_reviews(&project_a, &task_a).unwrap();
        let reviews_b = db
            .list_task_reviews(&project_b_row.id, &task_b_row.id)
            .unwrap();
        assert!(reviews_a.iter().all(|r| r.project_id == project_a));
        assert!(reviews_b.iter().all(|r| r.project_id == project_b_row.id));
        assert_ne!(reviews_a[0].verdict, reviews_b[0].verdict);

        let evidence_a = db.list_command_evidence_for_task(&task_a).unwrap();
        let evidence_b = db.list_command_evidence_for_task(&task_b_row.id).unwrap();
        assert!(evidence_a.iter().all(|e| e.task_id == task_a));
        assert!(evidence_b.iter().all(|e| e.task_id == task_b_row.id));

        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn reviewer_findings_are_redacted_before_being_persisted() {
        let dir = git_project_dir("redact-findings-core-loop");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1, 2]);
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success],
            vec![
                ReviewVerdict::ChangesRequired,
                ReviewVerdict::ChangesRequired,
            ],
        );
        *adapter.review_findings_text.lock().unwrap() =
            Some("Found an issue: Authorization: Bearer sk-fake-secret-1234567890".to_string());

        let outcome = run_core_loop_dual_ai_v0_2(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(
            outcome.task.state,
            TaskState::Failed,
            "two changes_required verdicts must exhaust the one fix pass and end in failed"
        );

        let reviews = db.list_task_reviews(&project_id, &task_id).unwrap();
        assert!(!reviews.is_empty());
        for review in &reviews {
            let findings = review.findings_summary.as_deref().unwrap_or("");
            assert!(
                !findings.contains("sk-fake-secret-1234567890"),
                "reviewer findings must be redacted before being persisted: {findings}"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn chinese_and_space_containing_project_paths_work_end_to_end() {
        let dir = git_project_dir("中文 项目 空格 core loop");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_transition_the_core_loop_performs_uses_a_backend_actor_never_user() {
        let dir = git_project_dir("actor-boundary");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success, MockOutcome::Success],
            vec![ReviewVerdict::ChangesRequired, ReviewVerdict::Approve],
        );
        run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        let events = db.list_task_state_events(&task_id).unwrap();
        // Every event written *by this run* (after the two initial
        // user-driven confirmation transitions) must be System/Nova, never
        // User — the core loop itself never has a way to impersonate the
        // user, matching the Tauri command boundary, which accepts only a
        // `task_id` and no actor field at all.
        let post_confirmation: Vec<_> = events
            .iter()
            .skip_while(|e| e.new_state != TaskState::Confirmed)
            .skip(1)
            .collect();
        assert!(!post_confirmation.is_empty());
        for event in post_confirmation {
            assert_ne!(
                event.actor,
                Actor::User,
                "core loop must never record a User-attributed transition"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Blocker 4: app-owned task package path safety ---------------------

    #[test]
    fn task_packages_are_written_outside_the_project_directory_entirely() {
        let dir = git_project_dir("blocker4-outside-project");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let app_data_dir = test_app_data_dir();

        run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();

        assert!(
            !dir.join(".nova").exists(),
            "no task package content must ever be written under the project's own .nova directory"
        );
        let packages_root = app_data_dir.join("task-packages");
        assert!(
            packages_root.join(&_project_id).join(&task_id).is_dir(),
            "task packages must land under <app-data>/task-packages/<project-id>/<task-id>/"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_docs_dir_rejects_a_project_id_containing_dot_dot_traversal() {
        let app_data_dir = test_app_data_dir();
        let result = task_docs_dir(&app_data_dir, "../escape", "task-1", 1);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn task_docs_dir_rejects_a_task_id_containing_a_path_separator() {
        let app_data_dir = test_app_data_dir();
        let result = task_docs_dir(&app_data_dir, "project-1", "..\\escape", 1);
        assert!(result.is_err());
        let result2 = task_docs_dir(&app_data_dir, "project-1", "sub/dir", 1);
        assert!(result2.is_err());
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn task_docs_dir_rejects_an_absolute_or_drive_prefixed_task_id() {
        let app_data_dir = test_app_data_dir();
        let result = task_docs_dir(&app_data_dir, "project-1", "C:\\Windows\\System32", 1);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn task_docs_dir_rejects_a_unc_style_task_id() {
        let app_data_dir = test_app_data_dir();
        let result = task_docs_dir(&app_data_dir, "project-1", "\\\\server\\share", 1);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn task_docs_dir_rejects_reserved_windows_device_names() {
        let app_data_dir = test_app_data_dir();
        for reserved in ["CON", "NUL", "con", "PRN", "COM1"] {
            let result = task_docs_dir(&app_data_dir, "project-1", reserved, 1);
            assert!(result.is_err(), "expected {reserved} to be rejected");
        }
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn task_docs_dir_rejects_trailing_dot_or_space_in_task_id() {
        let app_data_dir = test_app_data_dir();
        assert!(task_docs_dir(&app_data_dir, "project-1", "task-1.", 1).is_err());
        assert!(task_docs_dir(&app_data_dir, "project-1", "task-1 ", 1).is_err());
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    #[cfg(windows)]
    fn task_docs_dir_rejects_a_junction_ancestor_of_the_task_packages_root() {
        let app_data_dir = test_app_data_dir();
        let packages_root = app_data_dir.join("task-packages");
        fs::create_dir_all(&packages_root).unwrap();
        let real_target = temp_dir("blocker4-junction-target");
        let junction = packages_root.join("project-1");
        let status = StdCommand::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                junction.to_str().unwrap(),
                real_target.to_str().unwrap(),
            ])
            .status()
            .expect("run mklink");
        assert!(status.success(), "test setup: mklink /J must succeed");

        let result = task_docs_dir(&app_data_dir, "project-1", "task-1", 1);
        assert!(
            result.is_err(),
            "a junction planted as a task-packages ancestor must be rejected, not followed"
        );
        let _ = fs::remove_dir_all(&app_data_dir);
        let _ = fs::remove_dir_all(&real_target);
    }

    #[test]
    fn task_docs_dir_keeps_different_rounds_in_separate_directories() {
        let app_data_dir = test_app_data_dir();
        let dir1 = task_docs_dir(&app_data_dir, "project-1", "task-1", 1).unwrap();
        let dir2 = task_docs_dir(&app_data_dir, "project-1", "task-1", 2).unwrap();
        assert_ne!(dir1, dir2);
        let canonical_packages_root = fs::canonicalize(app_data_dir.join("task-packages")).unwrap();
        assert!(dir1.starts_with(&canonical_packages_root));
        assert!(dir2.starts_with(&canonical_packages_root));
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn task_docs_dir_is_safe_under_concurrent_creation_for_the_same_task() {
        let app_data_dir = Arc::new(test_app_data_dir());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let app_data_dir = Arc::clone(&app_data_dir);
                std::thread::spawn(move || task_docs_dir(&app_data_dir, "project-1", "task-1", 1))
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(
            results.iter().all(|r| r.is_ok()),
            "concurrent creation of the same task package directory must never fail: {results:?}"
        );
        let first = results[0].as_ref().unwrap();
        for result in &results {
            assert_eq!(result.as_ref().unwrap(), first);
        }
        let _ = fs::remove_dir_all(app_data_dir.as_ref());
    }

    // --- Major 4: core loop cancellation ------------------------------------

    #[test]
    fn cancelling_while_the_developer_ai_is_running_is_recorded_as_cancelled_not_failed() {
        let dir = git_project_dir("cancel-during-dev");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let cancel = CancellationToken::new();
        let cancel_for_effect = cancel.clone();
        *adapter.development_side_effect.lock().unwrap() =
            Some(Box::new(move |_req| cancel_for_effect.cancel()));

        let outcome =
            run_core_loop(&db, &task_id, &adapter, &cancel, &test_app_data_dir()).unwrap();

        assert_eq!(
            outcome.task.state,
            TaskState::Cancelled,
            "a cancellation mid-development must be recorded as cancelled, distinct from failed"
        );
        assert_eq!(
            adapter.review_call_count(),
            0,
            "a cancelled developer step must never reach the reviewer"
        );
        let events = db.list_task_state_events(&task_id).unwrap();
        assert_eq!(events.last().unwrap().new_state, TaskState::Cancelled);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_while_the_reviewer_ai_is_running_is_recorded_as_cancelled_not_needs_fix() {
        let dir = git_project_dir("cancel-during-review");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let cancel = CancellationToken::new();
        let cancel_for_effect = cancel.clone();
        *adapter.review_side_effect.lock().unwrap() =
            Some(Box::new(move |_req| cancel_for_effect.cancel()));

        let outcome =
            run_core_loop_dual_ai_v0_2(&db, &task_id, &adapter, &cancel, &test_app_data_dir())
                .unwrap();

        assert_eq!(outcome.task.state, TaskState::Cancelled);
        let reviews = db.list_task_reviews(&project_id, &task_id).unwrap();
        assert_eq!(
            reviews.len(),
            1,
            "a cancelled review round is still recorded as evidence, not silently dropped"
        );
        let review_key = format!("review-{task_id}-round-1");
        let action = db.find_action(&task_id, &review_key).unwrap().unwrap();
        assert_eq!(
            action.status,
            ActionStatus::Cancelled,
            "the review action must also land on `cancelled`, not `failed`"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_during_the_automatic_fix_pass_is_recorded_as_cancelled() {
        let dir = git_project_dir("cancel-during-safe-fixing");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1, 2]);
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success, MockOutcome::Success],
            vec![ReviewVerdict::ChangesRequired],
        );
        let cancel = CancellationToken::new();
        let cancel_for_effect = cancel.clone();
        let call_count = std::sync::atomic::AtomicUsize::new(0);
        *adapter.development_side_effect.lock().unwrap() = Some(Box::new(move |_req| {
            // Let round 1's development succeed normally; only cancel once
            // the automatic fix pass (round 2) starts its own development
            // step, proving `safe_fixing -> cancelled` (not just
            // `running -> cancelled`) is reachable and legal.
            if call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                cancel_for_effect.cancel();
            }
        }));

        let outcome =
            run_core_loop_dual_ai_v0_2(&db, &task_id, &adapter, &cancel, &test_app_data_dir())
                .unwrap();

        assert_eq!(outcome.task.state, TaskState::Cancelled);
        assert_eq!(adapter.development_call_count(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_cancellation_of_an_already_cancelled_task_does_not_panic_or_double_transition() {
        let dir = git_project_dir("repeated-cancel");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let cancel = CancellationToken::new();
        cancel.cancel();
        // Calling `.cancel()` again on an already-cancelled token (exactly
        // what a duplicate-click "cancel" button press would do) must be a
        // harmless no-op at the `CancellationToken` level.
        cancel.cancel();
        cancel.cancel();
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome =
            run_core_loop(&db, &task_id, &adapter, &cancel, &test_app_data_dir()).unwrap();
        assert_eq!(outcome.task.state, TaskState::Cancelled);

        // A second `run_core_loop` call against the now-`cancelled` (a
        // terminal state) task must not panic and must not be able to
        // silently resume the loop as if nothing happened.
        let second_result = run_core_loop(&db, &task_id, &adapter, &cancel, &test_app_data_dir());
        assert!(
            second_result.is_err(),
            "starting the loop again from a terminal `cancelled` state must be rejected, not silently resumed"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_one_projects_task_never_affects_a_concurrently_running_second_project() {
        let dir_a = git_project_dir("cancel-isolation-a");
        let dir_b = git_project_dir("cancel-isolation-b");
        let (db, _project_a, task_a) = db_with_confirmed_task(&dir_a, "codex_cli");
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
        let task_b = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project_b.id.clone(),
                    title: "B task".to_string(),
                    goal: Some("goal".to_string()),
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        db.transition_task(
            &task_b.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_b.id,
            TaskState::Confirmed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        confirm_no_tests(&db, &project_b.id, &task_b.id, 0, &[1]);

        // Task A's own cancellation token, already fired *before* A's loop
        // ever starts — task B uses a completely independent, never-fired
        // token and must reach success normally.
        let cancel_a = CancellationToken::new();
        cancel_a.cancel();
        let adapter_a = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let outcome_a =
            run_core_loop(&db, &task_a, &adapter_a, &cancel_a, &test_app_data_dir()).unwrap();

        let cancel_b = CancellationToken::new();
        let adapter_b = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let outcome_b =
            run_core_loop(&db, &task_b.id, &adapter_b, &cancel_b, &test_app_data_dir()).unwrap();

        assert_eq!(outcome_a.task.state, TaskState::Cancelled);
        assert_eq!(outcome_b.task.state, TaskState::AwaitingUserAcceptance);
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn a_cancelled_round_still_leaves_the_action_ledger_and_task_state_mutually_consistent() {
        let dir = git_project_dir("cancel-ledger-consistency");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let cancel = CancellationToken::new();
        let cancel_for_effect = cancel.clone();
        *adapter.development_side_effect.lock().unwrap() =
            Some(Box::new(move |_req| cancel_for_effect.cancel()));

        let outcome =
            run_core_loop(&db, &task_id, &adapter, &cancel, &test_app_data_dir()).unwrap();
        assert_eq!(outcome.task.state, TaskState::Cancelled);

        let dev_key = format!("dev-{task_id}-round-1");
        let action = db.find_action(&task_id, &dev_key).unwrap().unwrap();
        assert_eq!(
            action.status,
            ActionStatus::Cancelled,
            "re-review Major 4: a user cancellation must land on the ledger's own \
             `cancelled` terminal status, never be folded into `failed` alongside a \
             genuine process failure"
        );
        assert!(
            action
                .error_summary
                .as_deref()
                .is_some_and(|summary| summary.contains("cancellation")),
            "the ledger's error_summary must say this was a cancellation, not a generic failure: {:?}",
            action.error_summary
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_while_independent_tests_are_running_is_recorded_as_cancelled_not_failed() {
        // Re-review Major 4: the bug this closes lived specifically in the
        // test-execution round (`run_tests_for_round`'s cancellation branch
        // finished the ledger action as `Failed` even though the task
        // itself transitioned to `Cancelled`). Reaching that exact branch
        // requires a *real* allowlisted test command that is still running
        // when cancellation fires — `npm test` running a Node script that
        // sleeps a few seconds is the smallest such command available on
        // this dev machine (`node`/`npm` are already required for the
        // frontend build).
        //
        // Codex re-review remediation (Major 2): this test used to fire
        // cancellation from a timer thread after a fixed 800ms sleep,
        // racing against the main thread's own real progress through the
        // mock dev step, git evidence collection, and `gate_action`
        // planning the `test-{task_id}-round-1` action. Under full-suite
        // parallel CPU load, `MockAdapter::run_development` — which itself
        // checks `cancel.is_cancelled()` and short-circuits immediately —
        // could still be reached *after* the 800ms timer had already
        // fired, attributing the cancellation to the *development* step
        // instead of the intended test-execution step; the test-execution
        // action was then never planned at all, and
        // `db.find_action(...).unwrap().unwrap()` panicked on `None`. Fixed
        // by waiting for a real signal the spawned test process itself
        // writes to disk — by construction, this can only appear *after*
        // `run_tests_for_round`'s `gate_action` call already planned the
        // action and the real `node` child process actually started, so
        // cancellation is now deterministically attributed to the
        // test-execution step every time, regardless of scheduling delays.
        let dir = git_project_dir("cancel-during-test-execution");
        let marker = dir.join("test-started.marker");
        // v0.1.0 scope correction (Work Package 2 final stability
        // remediation): write to a temp name then rename — see the build
        // cancellation test's own comment for the full rationale (atomic
        // marker visibility) and `MARKER_WAIT_TIMEOUT`'s doc comment for
        // why the wait budget below was raised from a fixed 10 seconds.
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"require('fs').writeFileSync('test-started.marker.tmp','1');require('fs').renameSync('test-started.marker.tmp','test-started.marker');setTimeout(()=>process.exit(0), 6000)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();

        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let test_key = test_action_key(&task_id, 0, 1);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let cancel = CancellationToken::new();
        let cancel_for_timer = cancel.clone();
        let marker_for_timer = marker.clone();
        let dir_for_timer = dir.clone();
        let canceller = std::thread::spawn(move || {
            wait_for_marker_file(&marker_for_timer, &dir_for_timer, MARKER_WAIT_TIMEOUT);
            cancel_for_timer.cancel();
        });

        let outcome =
            run_core_loop(&db, &task_id, &adapter, &cancel, &test_app_data_dir()).unwrap();
        join_canceller_with_diagnostics(canceller, &db, &task_id, &test_key);

        assert_eq!(outcome.task.state, TaskState::Cancelled);
        let action = db.find_action(&task_id, &test_key).unwrap().unwrap();
        assert_eq!(
            action.status,
            ActionStatus::Cancelled,
            "the test-execution action must be `cancelled`, not `failed`, when the user \
             cancels while the independent test run is still in progress"
        );
        let evidence = db
            .list_test_evidence_for_round(&_project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].status, "cancelled");
        remove_dir_all_verifying_no_leftover_handles(&dir);
    }

    // --- Blocker 3: independent test execution --------------------------

    #[test]
    fn a_project_with_no_detectable_test_framework_and_no_user_confirmation_stops_for_the_user() {
        // Second re-review (Blocker 3): this replaces the prior version of
        // this test, which the re-review correctly called out as "proving
        // the unacceptable no-confirmation path" — Nova recording
        // `no_tests_applicable` and reaching `AwaitingUserAcceptance`
        // purely on its own detection failure, with no real user decision
        // anywhere in the loop. The corrected behavior: with no test
        // command and no confirmation, the loop must stop at
        // `NeedsUserAction` and must not fabricate any test evidence at
        // all — not even a `no_tests_applicable` row.
        let dir = git_project_dir("no-test-framework-unconfirmed");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::NeedsUserAction);
        let evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert!(
            evidence.is_empty(),
            "no test evidence — not even no_tests_applicable — may be recorded without a real confirmation: {evidence:?}"
        );
        let test_key = test_action_key(&task_id, 0, 1);
        let action = db.find_action(&task_id, &test_key).unwrap().unwrap();
        assert_eq!(action.status, ActionStatus::Failed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn confirming_no_tests_after_getting_stuck_actually_unblocks_a_resumed_run_to_success() {
        // Second re-review remediation: proves the *whole* real-world path,
        // not just that the confirmation record gets written — a user who
        // gets stuck at `needs_user_action`, confirms there are no
        // applicable tests, and clicks "start" again must actually see the
        // task complete, not stay stuck forever because the retried
        // `test-{task_id}-round-1` action was left in a terminal `failed`
        // status a plain `start_action` can never leave again.
        let dir = git_project_dir("no-test-framework-unblock-resume");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let app_data_dir = test_app_data_dir();

        let stuck = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();
        assert_eq!(stuck.task.state, TaskState::NeedsUserAction);

        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, Some("no tests yet"))
            .unwrap();
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, Some("no build yet"))
            .unwrap();
        let unblocked = unblock_test_execution_for_retry(&db, &project_id, &task_id).unwrap();
        assert_eq!(unblocked.state, TaskState::Queued);
        let test_key = test_action_key(&task_id, 0, 1);
        assert_eq!(
            db.find_action(&task_id, &test_key).unwrap().unwrap().status,
            ActionStatus::Planned,
            "the stalled test action must be back to planned, retryable again"
        );

        let resumed = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &app_data_dir,
        )
        .unwrap();
        assert_eq!(
            resumed.task.state,
            TaskState::AwaitingUserAcceptance,
            "the resumed run must actually complete the round this time, not get stuck again"
        );
        let evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].status, "no_tests_applicable");
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Codex re-review remediation (Major 1): pending round is an
    // Action Ledger fact, never a `task_reviews` inference ------------

    #[test]
    fn current_pending_round_is_1_while_genuinely_stuck_on_round_1s_own_no_tests_confirmation() {
        let dir = git_project_dir("pending-round-stuck-on-round-1");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);

        let stuck = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(stuck.task.state, TaskState::NeedsUserAction);

        assert_eq!(
            current_pending_round(&db, &project_id, &task_id).unwrap(),
            1,
            "no round-2 Action Ledger entry exists yet — must report round 1"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_pending_round_is_2_after_round_1_genuinely_fails_and_round_2_gets_its_own_stuck_confirmation_and_resumes_without_repeating_round_1(
    ) {
        // Codex re-review remediation (Major 1): the concrete bug this
        // closes — a single-AI task whose round 1 genuinely fails (real
        // test failure, not a stall), triggers the one automatic fix pass,
        // and then round 2 *itself* gets stuck needing its own no-tests
        // confirmation (the round-2 fix removed the test framework
        // entirely). The old `task_reviews`-based logic would have reported
        // round 1 here forever (single-AI tasks never write `task_reviews`)
        // — confirming under that stale round would target the wrong
        // Action Ledger key and never actually unblock the real stuck
        // round.
        let dir = git_project_dir("pending-round-stuck-on-round-2");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(1)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add failing package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();

        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);
        // Round 2's development "fix" deletes package.json entirely — a
        // real, observable file change (not just a scripted verdict) that
        // makes round 2's own independent test-command detection come back
        // empty, so round 2 must stall on its *own* no-tests confirmation
        // rather than reusing round 1's already-resolved command.
        let call_count = std::sync::atomic::AtomicUsize::new(0);
        let package_json = dir.join("package.json");
        let dir_for_effect = dir.clone();
        *adapter.development_side_effect.lock().unwrap() = Some(Box::new(move |_req| {
            if call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                let _ = fs::remove_file(&package_json);
                // A real developer-AI fix leaves the working tree clean
                // (committed) — otherwise the dirty-worktree guard (which
                // runs unconditionally on every `run_core_loop` invocation,
                // including a resume) would itself stall the task, which
                // is not what this test is about.
                let _ = StdCommand::new("git")
                    .args(["add", "-A"])
                    .current_dir(&dir_for_effect)
                    .status();
                let _ = StdCommand::new("git")
                    .args(["commit", "-q", "-m", "round 2 fix: remove package.json"])
                    .current_dir(&dir_for_effect)
                    .status();
            }
        }));

        let stuck = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(
            stuck.task.state,
            TaskState::NeedsUserAction,
            "round 2 must stall on its own missing test command, not silently reuse round 1's"
        );
        assert_eq!(
            adapter.development_call_count(),
            2,
            "round 1's real test failure must have triggered exactly one automatic fix pass"
        );

        assert_eq!(
            current_pending_round(&db, &project_id, &task_id).unwrap(),
            2,
            "a real round-2 dev/test Action Ledger entry already exists — must report round 2, \
             never fall back to round 1 the way the old task_reviews-based logic did"
        );

        db.record_no_tests_confirmation(&project_id, &task_id, 0, 2, Some("removed by the fix"))
            .unwrap();
        db.record_no_build_confirmation(&project_id, &task_id, 0, 2, Some("removed by the fix"))
            .unwrap();
        let unblocked = unblock_test_execution_for_retry(&db, &project_id, &task_id).unwrap();
        assert_eq!(unblocked.state, TaskState::Queued);
        let round1_test_key = test_action_key(&task_id, 0, 1);
        assert_eq!(
            db.find_action(&task_id, &round1_test_key)
                .unwrap()
                .unwrap()
                .status,
            ActionStatus::Failed,
            "unblocking round 2 must never reset round 1's own already-terminal action"
        );

        let resumed = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(
            resumed.task.state,
            TaskState::AwaitingUserAcceptance,
            "the resumed round 2 must actually complete, not get stuck again"
        );
        assert_eq!(
            adapter.development_call_count(),
            2,
            "resuming round 2 must never repeat round 1's development or invoke a third \
             (paid) development call"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_pending_round_ignores_a_stale_dual_ai_era_task_reviews_row() {
        // A round-1 `task_reviews` row (old dual-AI/dormant-path data) must
        // never, by itself, make `current_pending_round` report round 2 for
        // a single-AI task that has not actually started a round 2 in the
        // Action Ledger.
        let dir = git_project_dir("pending-round-ignores-stale-review");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.record_task_review(
            &project_id,
            &task_id,
            0,
            1,
            "claude_cli",
            "approve",
            Some("stale dual-AI era row"),
        )
        .unwrap();

        assert_eq!(
            current_pending_round(&db, &project_id, &task_id).unwrap(),
            1,
            "a task_reviews row must never drive this decision for the single-AI path"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_with_no_detectable_test_framework_and_a_real_confirmation_proceeds_and_succeeds() {
        let dir = git_project_dir("no-test-framework-confirmed");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, Some("brand new project"))
            .unwrap();
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, Some("brand new project"))
            .unwrap();
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        let evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].status, "no_tests_applicable");
        assert!(
            evidence[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("用户已确认")),
            "the evidence's own reason must reference the real user confirmation: {:?}",
            evidence[0].reason
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_no_tests_confirmation_from_a_prior_round_does_not_carry_forward_to_the_fix_pass_round() {
        // The confirmation is scoped to round 1 only; round 2's own test
        // execution (the automatic fix pass) must independently require its
        // own confirmation rather than silently inheriting round 1's.
        let dir = git_project_dir("no-test-framework-stale-confirmation");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success, MockOutcome::Success],
            vec![ReviewVerdict::ChangesRequired],
        );

        let outcome = run_core_loop_dual_ai_v0_2(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(
            outcome.task.state,
            TaskState::NeedsUserAction,
            "round 2's test execution must stop for a fresh confirmation, not reuse round 1's"
        );
        let round_2_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 2)
            .unwrap();
        assert!(round_2_evidence.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_configured_with_a_non_allowlisted_test_command_is_refused_before_execution() {
        let dir = git_project_dir("non-allowlisted-command");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.set_task_test_command(&task_id, "bash -c \"rm -rf /\"", "user")
            .unwrap();
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(
            outcome.task.state,
            TaskState::NeedsUserAction,
            "a non-allowlisted test command must never be executed, not even a user-typed one"
        );
        assert_eq!(
            adapter.review_call_count(),
            0,
            "review must never be reached when the test command fails the allowlist check"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_round_produces_its_own_independent_test_evidence_never_reusing_an_earlier_round() {
        let dir = git_project_dir("test-evidence-per-round");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1, 2]);
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success, MockOutcome::Success],
            vec![ReviewVerdict::ChangesRequired, ReviewVerdict::Approve],
        );

        let outcome = run_core_loop_dual_ai_v0_2(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();
        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);

        let round1_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        let round2_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 2)
            .unwrap();
        assert_eq!(round1_evidence.len(), 1);
        assert_eq!(round2_evidence.len(), 1);
        assert_ne!(
            round1_evidence[0].id, round2_evidence[0].id,
            "round 2 must produce a fresh test-evidence row, never reuse round 1's"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Work Package 3: generation-aware test action key isolation ------
    //
    // `CODEX_PHASE3_SINGLE_AI_UI_REPORT_SCOPE_CORRECTION_REMEDIATION_
    // REREVIEW09241542.md` Minor 2 / `CODEX_PHASE3_BUILD_CANCELLATION_
    // STABILITY_FINAL_REVIEW09241759.md` Minor 3: the test Action Ledger
    // key is now generation-aware (`test_action_key`), mirroring
    // `build_action_key`'s own Major 3 remediation. The tests below prove
    // the full required matrix: distinct keys per generation/round, a
    // succeeded-but-evidence-less action always fails closed (never
    // `NoTestsApplicable`), a terminal action from a foreign generation
    // never pollutes the current one, historical non-generation-aware keys
    // are read safely without crashing, a new generation genuinely
    // re-executes tests, and `Completed` remains bound to the task's
    // *current* `result_generation`.

    #[test]
    fn test_action_key_differs_across_generations_for_the_same_task_and_round() {
        let key_gen0 = test_action_key("task-1", 0, 1);
        let key_gen1 = test_action_key("task-1", 1, 1);
        assert_ne!(
            key_gen0, key_gen1,
            "a generation-1 test action must never collide with generation 0's key"
        );
    }

    #[test]
    fn test_action_key_differs_across_rounds_for_the_same_task_and_generation() {
        let key_round1 = test_action_key("task-1", 0, 1);
        let key_round2 = test_action_key("task-1", 0, 2);
        assert_ne!(
            key_round1, key_round2,
            "round 2 must never collide with round 1's key under the same generation"
        );
    }

    /// Drives a freshly `confirmed` task's `result_generation` up to
    /// `target` using only real, legal [`Database::transition_task`] calls
    /// — `Verifying -> PreviewReady` is the *only* edge that ever bumps
    /// `result_generation` (see the inline comment on that `UPDATE` in
    /// `storage::tasks`), so this never writes to the column directly, the
    /// same discipline `documents.rs`'s own generation tests already
    /// follow. `Actor::User` is used throughout (including the
    /// `SafeFixing` hop a second bump needs) purely so this setup helper
    /// is never itself constrained by the automatic-fix-cycle cap that
    /// only applies to non-`User` actors.
    fn bump_result_generation_via_real_transitions(db: &Database, task_id: &str, target: i64) {
        let mut current = db.task(task_id).unwrap().result_generation;
        assert!(
            current <= target,
            "helper only ever moves a task's generation forward"
        );
        if current < target {
            for to in [
                TaskState::Queued,
                TaskState::Running,
                TaskState::Verifying,
                TaskState::PreviewReady,
            ] {
                db.transition_task(task_id, to, Actor::User, None, None, false, None)
                    .unwrap();
            }
            current += 1;
        }
        while current < target {
            for to in [
                TaskState::SafeFixing,
                TaskState::Verifying,
                TaskState::PreviewReady,
            ] {
                db.transition_task(task_id, to, Actor::User, None, None, false, None)
                    .unwrap();
            }
            current += 1;
        }
    }

    #[test]
    fn generation_1_test_action_succeeded_never_lets_generation_0_pass_without_its_own_evidence() {
        // Mirrors `generation_1_build_succeeded_never_lets_generation_2_
        // pass_without_its_own_evidence` for the test action key.
        let dir = git_project_dir("test-gen-mismatch-fail-closed");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);

        // Simulate generation 1's test action already marked succeeded,
        // with no corresponding test_evidence row for generation 1.
        let stale_key = test_action_key(&task_id, 1, 1);
        db.plan_action(&task_id, "test_execution", &stale_key)
            .unwrap();
        db.start_action(&task_id, &stale_key, false).unwrap();
        db.finish_action(&task_id, &stale_key, ActionStatus::Succeeded, None, None)
            .unwrap();
        // Deliberately no `record_test_evidence` call for generation 1.

        // The task's actual current `result_generation` is still 0 — a
        // call for generation 0 must go through the normal `Started` path
        // (a fresh key), completely unaffected by generation 1's stale,
        // evidence-less succeeded action.
        let result = run_tests_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(
            matches!(result, TestRoundOutcome::NoTestsApplicable(_)),
            "generation 0's own real no-tests confirmation must apply unaffected by generation \
             1's unrelated stale action"
        );
        let gen0_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(
            gen0_evidence.len(),
            1,
            "generation 0 must have gotten its own evidence, not reused generation 1's"
        );
        let gen1_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 1, 1)
            .unwrap();
        assert!(
            gen1_evidence.is_empty(),
            "generation 1's stale succeeded action must never have produced test evidence"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn already_succeeded_test_action_with_no_matching_evidence_fails_closed() {
        // Directly exercises the `AlreadySucceeded` + missing-evidence
        // branch: the action ledger says generation 0/round 1's test
        // succeeded, but no test_evidence row exists for that exact pair
        // — must record `insufficient_evidence`, transition to
        // `NeedsUserAction`, and never report `NoTestsApplicable`.
        let dir = git_project_dir("test-already-succeeded-no-evidence");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let key = test_action_key(&task_id, 0, 1);
        db.plan_action(&task_id, "test_execution", &key).unwrap();
        db.start_action(&task_id, &key, false).unwrap();
        db.finish_action(&task_id, &key, ActionStatus::Succeeded, None, None)
            .unwrap();

        let result = run_tests_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();

        assert!(
            matches!(result, TestRoundOutcome::ActionUncertain),
            "a succeeded test action with no matching evidence must fail closed, not report \
             NoTestsApplicable"
        );
        let evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].status, "insufficient_evidence");
        assert_eq!(db.task(&task_id).unwrap().state, TaskState::NeedsUserAction);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn already_succeeded_test_action_with_only_a_prior_generations_evidence_fails_closed() {
        // Distinct from the mismatch test above: here real, genuine
        // `passed` evidence *does* exist — just for the wrong generation
        // (0), while the action ledger and the call are both for
        // generation 1. A prior generation's real evidence must never
        // substitute for the current one's.
        let dir = git_project_dir("test-only-prior-generation-evidence");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
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
        bump_result_generation_via_real_transitions(&db, &task_id, 1);
        assert_eq!(db.task(&task_id).unwrap().result_generation, 1);
        // The fail-closed branch below transitions to `needs_user_action`,
        // which is only legal from `running`/`verifying`/`safe_fixing`/
        // `awaiting_user_acceptance` — move off `preview_ready` first via
        // the same real, legal edges a genuine "request more changes"
        // flow would use.
        db.transition_task(
            &task_id,
            TaskState::SafeFixing,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let key = test_action_key(&task_id, 1, 1);
        db.plan_action(&task_id, "test_execution", &key).unwrap();
        db.start_action(&task_id, &key, false).unwrap();
        db.finish_action(&task_id, &key, ActionStatus::Succeeded, None, None)
            .unwrap();

        let result = run_tests_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(
            matches!(result, TestRoundOutcome::ActionUncertain),
            "generation 0's real passed evidence must never satisfy generation 1's own gate"
        );
        let gen1_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 1, 1)
            .unwrap();
        assert_eq!(gen1_evidence.len(), 1);
        assert_eq!(gen1_evidence[0].status, "insufficient_evidence");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn already_succeeded_test_action_with_only_a_prior_rounds_evidence_fails_closed() {
        // Same shape as the generation test above, but across rounds
        // within the same generation: round 1's real `passed` evidence
        // must never satisfy round 2's own gate.
        let dir = git_project_dir("test-only-prior-round-evidence");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
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
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let key = test_action_key(&task_id, 0, 2);
        db.plan_action(&task_id, "test_execution", &key).unwrap();
        db.start_action(&task_id, &key, false).unwrap();
        db.finish_action(&task_id, &key, ActionStatus::Succeeded, None, None)
            .unwrap();

        let result = run_tests_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            2,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(
            matches!(result, TestRoundOutcome::ActionUncertain),
            "round 1's real passed evidence must never satisfy round 2's own gate"
        );
        let round2_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 2)
            .unwrap();
        assert_eq!(round2_evidence.len(), 1);
        assert_eq!(round2_evidence[0].status, "insufficient_evidence");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_cancelled_or_uncertain_test_action_from_a_foreign_generation_never_blocks_a_fresh_action(
    ) {
        let dir = git_project_dir("test-foreign-generation-terminal-status");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);

        // Foreign generation 1: a genuine test failure.
        let failed_key = test_action_key(&task_id, 1, 1);
        db.plan_action(&task_id, "test_execution", &failed_key)
            .unwrap();
        db.start_action(&task_id, &failed_key, false).unwrap();
        db.finish_action(
            &task_id,
            &failed_key,
            ActionStatus::Failed,
            None,
            Some("unrelated failure"),
        )
        .unwrap();

        // Foreign generation 2: a user cancellation.
        let cancelled_key = test_action_key(&task_id, 2, 1);
        db.plan_action(&task_id, "test_execution", &cancelled_key)
            .unwrap();
        db.start_action(&task_id, &cancelled_key, false).unwrap();
        db.finish_action(
            &task_id,
            &cancelled_key,
            ActionStatus::Cancelled,
            None,
            None,
        )
        .unwrap();

        // Foreign generation 3: a crash-recovery sweep leaves it uncertain.
        let uncertain_key = test_action_key(&task_id, 3, 1);
        db.plan_action(&task_id, "test_execution", &uncertain_key)
            .unwrap();
        db.start_action(&task_id, &uncertain_key, false).unwrap();
        db.mark_started_actions_uncertain().unwrap();
        assert_eq!(
            db.find_action(&task_id, &uncertain_key)
                .unwrap()
                .unwrap()
                .status,
            ActionStatus::Uncertain
        );

        // The task's real, current generation is still 0 — none of the
        // three foreign-generation terminal statuses above may affect it.
        let result = run_tests_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(
            matches!(result, TestRoundOutcome::NoTestsApplicable(_)),
            "generation 0's own fresh action must plan/start normally regardless of any \
             unrelated foreign generation's failed/cancelled/uncertain action: {result:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pre_remediation_non_generation_aware_test_action_key_is_read_safely_and_never_reused() {
        // Work Package 3 backward-compatibility requirement: a historical
        // Action Ledger row written before this remediation (key shape
        // `test-{task_id}-round-{round}`, no `-generation-` segment) must
        // never crash the loop and must never be mistaken for the current
        // generation-aware key — the two are simply different strings
        // under the same `UNIQUE(task_id, idempotency_key)` constraint, so
        // no explicit migration or parsing of the old shape is needed.
        let dir = git_project_dir("test-legacy-key-compat");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);

        let legacy_key = format!("test-{task_id}-round-1");
        db.plan_action(&task_id, "test_execution", &legacy_key)
            .unwrap();
        db.start_action(&task_id, &legacy_key, false).unwrap();
        db.finish_action(&task_id, &legacy_key, ActionStatus::Succeeded, None, None)
            .unwrap();

        let result = run_tests_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(
            matches!(result, TestRoundOutcome::NoTestsApplicable(_)),
            "the current generation-aware key must plan/start fresh, never reusing the legacy \
             row's status: {result:?}"
        );

        let all_actions = db.list_actions_for_task(&task_id).unwrap();
        assert_eq!(
            all_actions.len(),
            2,
            "both the legacy row and the new generation-aware row must remain independently \
             readable, not merged or dropped: {all_actions:?}"
        );
        let new_key = test_action_key(&task_id, 0, 1);
        assert!(all_actions.iter().any(|a| a.idempotency_key == legacy_key));
        assert!(all_actions.iter().any(|a| a.idempotency_key == new_key));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_generation_genuinely_re_executes_tests_and_produces_independent_passed_evidence() {
        let dir = git_project_dir("test-new-generation-reexecutes");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","version":"1.0.0","scripts":{"test":"node -e \"process.exit(0)\""}}"#,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "package.json"])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add passing test"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");

        let gen0_result = run_tests_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(matches!(gen0_result, TestRoundOutcome::Passed(_)));

        bump_result_generation_via_real_transitions(&db, &task_id, 1);
        assert_eq!(db.task(&task_id).unwrap().result_generation, 1);

        let gen1_result = run_tests_for_round(
            &db,
            &project_id,
            &task_id,
            &dir,
            1,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(
            matches!(gen1_result, TestRoundOutcome::Passed(_)),
            "generation 1's own round 1 must genuinely re-execute the real test command, not \
             short-circuit on generation 0's unrelated key: {gen1_result:?}"
        );

        let gen0_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        let gen1_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 1, 1)
            .unwrap();
        assert_eq!(gen0_evidence.len(), 1);
        assert_eq!(gen1_evidence.len(), 1);
        assert_eq!(gen0_evidence[0].status, "passed");
        assert_eq!(gen1_evidence[0].status, "passed");
        assert_ne!(
            gen0_evidence[0].id, gen1_evidence[0].id,
            "generation 1 must produce its own fresh evidence row, never reuse generation 0's"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn completed_transition_requires_acceptance_for_the_tasks_current_generation_not_an_earlier_one(
    ) {
        // Items 15/16 of the Work Package 3 generation matrix: user
        // acceptance must bind to `result_generation`, and a fresh
        // generation invalidates an earlier one's acceptance. Uses only
        // real `Database::transition_task`/`record_user_acceptance` calls
        // — the same completion gate Blocker 1 already bound to
        // `result_generation` (`storage::tasks::transition_task`'s
        // `completed` `WHERE` clause), exercised here specifically through
        // a *second* generation produced after the first was accepted.
        let dir = git_project_dir("test-generation-acceptance-binding");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");

        bump_result_generation_via_real_transitions(&db, &task_id, 1);
        assert_eq!(db.task(&task_id).unwrap().result_generation, 1);
        db.transition_task(
            &task_id,
            TaskState::AwaitingUserAcceptance,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.record_user_acceptance(&project_id, &task_id, true, None)
            .unwrap();

        // A second generation is produced (e.g. the user asked for more
        // changes after all) without ever completing generation 1's
        // acceptance into `completed`.
        db.transition_task(
            &task_id,
            TaskState::SafeFixing,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::PreviewReady,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        assert_eq!(db.task(&task_id).unwrap().result_generation, 2);
        db.transition_task(
            &task_id,
            TaskState::AwaitingUserAcceptance,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let rejected = db.transition_task(
            &task_id,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        );
        assert!(
            rejected.is_err(),
            "generation 1's old acceptance must never authorize generation 2's completion: \
             {rejected:?}"
        );
        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::AwaitingUserAcceptance,
            "a refused completion must leave the task exactly where it was"
        );

        db.record_user_acceptance(&project_id, &task_id, true, None)
            .unwrap();
        let completed = db
            .transition_task(
                &task_id,
                TaskState::Completed,
                Actor::User,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        assert_eq!(
            completed.state,
            TaskState::Completed,
            "a real, current-generation acceptance must authorize completion"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finish_success_path_refuses_to_advance_without_current_round_test_evidence() {
        // Direct unit test of the defense-in-depth gate itself: even if
        // some future code path reached `finish_success_path` without ever
        // having run `run_tests_for_round`, it must still refuse.
        let dir = git_project_dir("finish-success-path-gate");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        db.transition_task(
            &task_id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Verifying,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        let result = finish_success_path(&db, &project_id, &task_id, 1);
        assert!(
            result.is_err(),
            "must refuse to enter preview_ready without a passed/no_tests_applicable test-evidence row for this round"
        );
        assert_eq!(
            db.task(&task_id).unwrap().state,
            TaskState::Verifying,
            "a refused finish_success_path must not have transitioned the task at all"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_cargo_test_failure_is_independently_detected_and_blocks_preview_ready() {
        // A minimal, genuinely buildable crate with one failing test —
        // proves Nova's own independent `cargo test` run (not anything the
        // mock developer AI claims) is what actually gates the round, using
        // the real process boundary end to end.
        let dir = git_project_dir("real-cargo-test-failure");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"nova-fixture-failing\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("src").join("lib.rs"),
            "#[test]\nfn this_test_always_fails() {\n    assert_eq!(1, 2);\n}\n",
        )
        .unwrap();
        // The dirty-worktree guard runs before any AI/test step — these new
        // files must be committed, not left untracked, or the loop would
        // stop at `needs_user_action` before ever reaching test execution.
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "add failing test fixture"])
            .current_dir(&dir)
            .status()
            .unwrap();
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        assert_eq!(
            outcome.task.state,
            TaskState::Failed,
            "a real, independently-detected test failure must exhaust the one automatic fix pass and block preview_ready"
        );
        assert_eq!(
            adapter.review_call_count(),
            0,
            "the reviewer must never even be invoked when Nova's own test run genuinely failed"
        );
        let round1_evidence = db
            .list_test_evidence_for_round(&project_id, &task_id, 0, 1)
            .unwrap();
        assert_eq!(round1_evidence[0].status, "failed");
        assert!(round1_evidence[0].failed_count >= 1);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Minors --------------------------------------------------------

    #[test]
    fn git_output_classified_distinguishes_not_a_git_repository_from_other_failures() {
        let dir = temp_dir("git-classify-not-a-repo");
        // A plain directory, never `git init`-ed.
        let outcome =
            git_output_classified(&dir, &["status", "--porcelain"], &CancellationToken::new());
        assert_eq!(outcome, GitOutputOutcome::NotAGitRepository);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_output_classified_distinguishes_a_missing_project_directory() {
        let dir = temp_dir("git-classify-missing");
        let missing = dir.join("does-not-exist");
        let outcome = git_output_classified(
            &missing,
            &["status", "--porcelain"],
            &CancellationToken::new(),
        );
        assert_eq!(outcome, GitOutputOutcome::ProjectDirectoryMissing);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_output_classified_reports_success_with_real_output_for_a_real_repo() {
        let dir = git_project_dir("git-classify-success");
        let outcome =
            git_output_classified(&dir, &["rev-parse", "HEAD"], &CancellationToken::new());
        match outcome {
            GitOutputOutcome::Success(stdout) => assert!(!stdout.trim().is_empty()),
            other => panic!("expected Success, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_evidence_stores_a_real_redacted_stdout_and_stderr_reference_not_always_none() {
        // v0.1.0 single-AI path: the only command-evidence-producing step
        // is the developer AI's own invocation (there is no reviewer step
        // to fall back on) — see the matching change to `MockAdapter::
        // run_development`'s synthetic stdout.
        let dir = git_project_dir("command-evidence-refs");
        let (db, project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        confirm_no_tests(&db, &project_id, &task_id, 0, &[1]);
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        run_core_loop(
            &db,
            &task_id,
            &adapter,
            &CancellationToken::new(),
            &test_app_data_dir(),
        )
        .unwrap();

        let evidence = db.list_command_evidence_for_task(&task_id).unwrap();
        assert!(!evidence.is_empty());
        assert!(
            evidence.iter().any(|e| e.stdout_ref.is_some()),
            "at least one command_evidence row must carry a real stdout_ref, never always None"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
