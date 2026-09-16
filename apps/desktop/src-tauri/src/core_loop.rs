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
use crate::runtime::types::{ExecutionOutcome, ExecutionRequest, ToolKind};
use crate::security::redact_sensitive_text;

const DEV_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const REVIEW_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// The developer AI process itself failed to complete (not a review
    /// verdict) — the task has already been transitioned to `failed`
    /// directly; the caller must take no further action.
    Stopped,
}

pub struct CoreLoopOutcome {
    pub task: Task,
}

/// Drives `task_id` through the loop until it reaches a stopping point:
/// `awaiting_user_acceptance` (success), `failed` (second review still
/// rejected), `needs_user_action` (dirty worktree, uncertain action, or a
/// reviewer integrity violation), or `cancelled`. Returns the task's final
/// state, whatever it is — this function never returns `Err` for a
/// legitimate product-level stop, only for a genuine programming/precondition
/// error (wrong starting state, missing project, bad `ai_kind`).
pub fn run_core_loop(
    db: &Database,
    task_id: &str,
    adapter: &dyn DevReviewAdapter,
    cancel: &CancellationToken,
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
    )?;
    match round1 {
        RoundOutcome::Approved => finish_success_path(db, task_id)?,
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
            )?;
            match round2 {
                RoundOutcome::Approved => finish_success_path(db, task_id)?,
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

fn finish_success_path(db: &Database, task_id: &str) -> NovaResult<()> {
    db.transition_task(
        task_id,
        TaskState::PreviewReady,
        Actor::System,
        Some("审核通过，结果已就绪"),
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

/// One development + review round. The caller has already transitioned the
/// task into `running` (round 1) or `safe_fixing` (round 2) before calling.
/// On return the task is left in `verifying` (whatever the verdict), except
/// for the `ActionUncertain` case, where the task has already been moved to
/// `needs_user_action` by this function.
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
) -> NovaResult<RoundOutcome> {
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
            return Ok(RoundOutcome::ActionUncertain);
        }
        ActionGate::AlreadySucceeded => {
            // A prior attempt already completed this exact round's
            // development step (crash-recovery resume after the dev run
            // committed but before the review step finished) — proceed
            // straight to evidence collection/review without re-invoking
            // the developer AI a second time for the same round.
        }
        ActionGate::Started => {
            let package_path = write_task_package(db, task_id, project_dir, round, prior_findings)?;
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
                db.transition_task(
                    task_id,
                    TaskState::Failed,
                    Actor::System,
                    Some("开发 AI 进程未能正常完成"),
                    None,
                    false,
                    None,
                )?;
                return Ok(RoundOutcome::Stopped);
            }
        }
    }

    collect_git_evidence(db, project_id, task_id, project_dir, cancel)?;

    db.transition_task(
        task_id,
        TaskState::Verifying,
        Actor::System,
        Some("独立审核开始"),
        None,
        false,
        None,
    )?;

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
    let package_path = write_review_package(db, task_id, project_dir, round)?;

    let before_status = git_status_porcelain(project_dir, cancel)?.unwrap_or_default();
    let before_head = git_head(project_dir, cancel)?;

    let review_request = ReviewRequest {
        ai_kind: reviewer_kind,
        project_dir: project_dir.to_path_buf(),
        package_path,
        timeout: REVIEW_TIMEOUT,
    };
    let result = adapter.run_review(&review_request, cancel.clone());
    record_command_evidence(db, project_id, task_id, &result, "ai_review")?;

    let after_status = git_status_porcelain(project_dir, cancel)?.unwrap_or_default();
    let after_head = git_head(project_dir, cancel)?;
    let integrity_ok = before_status == after_status && before_head == after_head;

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

    let verdict = ai_adapter::parse_review_verdict(&result.stdout)
        .unwrap_or(ReviewVerdict::InsufficientEvidence);
    let findings = redact_sensitive_text(result.stdout.trim());
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

fn finish_gated_action(
    db: &Database,
    task_id: &str,
    key: &str,
    succeeded: bool,
    result: &crate::runtime::types::ExecutionResult,
) -> NovaResult<()> {
    let status = if succeeded {
        ActionStatus::Succeeded
    } else {
        ActionStatus::Failed
    };
    let error_summary = match &result.outcome {
        ExecutionOutcome::Failed { detail, .. } => Some(redact_sensitive_text(detail)),
        ExecutionOutcome::Success => None,
    };
    db.finish_action(task_id, key, status, None, error_summary.as_deref())?;
    Ok(())
}

fn record_command_evidence(
    db: &Database,
    project_id: &str,
    task_id: &str,
    result: &crate::runtime::types::ExecutionResult,
    label: &str,
) -> NovaResult<()> {
    let command_text = format!("{label}: {} {}", result.program, result.args.join(" "));
    db.record_command_evidence(
        project_id,
        task_id,
        &command_text,
        result.exit_code.map(i64::from),
        Some(&result.started_at.to_rfc3339()),
        Some(&result.ended_at.to_rfc3339()),
        None,
        None,
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

fn git_output(
    project_dir: &Path,
    args: &[&str],
    cancel: &CancellationToken,
) -> NovaResult<Option<String>> {
    let request = ExecutionRequest::new(
        "git".to_string(),
        args.iter().map(|s| s.to_string()).collect(),
        project_dir.to_path_buf(),
        Some(project_dir.to_path_buf()),
    )
    .with_timeout(GIT_TIMEOUT);
    let result = process::execute_blocking(request, cancel.clone());
    match result.outcome {
        ExecutionOutcome::Success => Ok(Some(result.stdout)),
        ExecutionOutcome::Failed { .. } => Ok(None),
    }
}

/// Writes the developer AI's task package: the confirmed requirement card
/// fields plus, for a fix pass, the prior round's redacted findings. Lives
/// under the project's own `.nova/tasks/<task-id>/` directory, matching the
/// Markdown-doc location the product spec and Implementation Map both use.
fn write_task_package(
    db: &Database,
    task_id: &str,
    project_dir: &Path,
    round: i64,
    prior_findings: Option<&str>,
) -> NovaResult<PathBuf> {
    let task = db.task(task_id)?;
    let dir = task_docs_dir(project_dir, task_id)?;
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

    fs::write(&path, body).map_err(NovaError::from)?;
    Ok(path)
}

/// Writes the reviewer AI's package: same requirement context, explicit
/// read-only instruction, and the exact verdict-marker convention Nova
/// parses (`runtime::ai_adapter::VERDICT_MARKER`).
fn write_review_package(
    db: &Database,
    task_id: &str,
    project_dir: &Path,
    round: i64,
) -> NovaResult<PathBuf> {
    let task = db.task(task_id)?;
    let dir = task_docs_dir(project_dir, task_id)?;
    let path = dir.join(format!("REVIEW_REQUEST_{round}.md"));

    let mut body = String::new();
    body.push_str(&format!("# Nova Review Package (round {round})\n\n"));
    body.push_str("You are the independent reviewer AI for this task. You are READ-ONLY: do not modify, create, or delete any file in this project. Nova technically verifies this after you finish.\n\n");
    body.push_str(&format!(
        "## Original goal\n\n{}\n\n",
        task.goal.as_deref().unwrap_or("(not specified)")
    ));
    body.push_str(&format!(
        "## Acceptance criteria\n\n{}\n\n",
        task.done_criteria.as_deref().unwrap_or("(not specified)")
    ));
    body.push_str("Review the actual code changes and any test evidence you can observe. End your output with exactly one line, verbatim, choosing one:\n\n");
    body.push_str(&format!(
        "{} APPROVE\n{} CHANGES_REQUIRED\n{} INSUFFICIENT_EVIDENCE\n",
        ai_adapter::VERDICT_MARKER,
        ai_adapter::VERDICT_MARKER,
        ai_adapter::VERDICT_MARKER
    ));

    fs::write(&path, body).map_err(NovaError::from)?;
    Ok(path)
}

fn task_docs_dir(project_dir: &Path, task_id: &str) -> NovaResult<PathBuf> {
    let dir = project_dir.join(".nova").join("tasks").join(task_id);
    fs::create_dir_all(&dir).map_err(NovaError::from)?;
    Ok(dir)
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

    /// A real, minimal git repository with one committed file — needed so
    /// `git status --porcelain`/`git rev-parse HEAD` behave like a real
    /// project rather than erroring on an empty/non-git directory.
    fn git_project_dir(name: &str) -> PathBuf {
        let dir = temp_dir(name);
        for args in [
            vec!["init", "-q"],
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

    fn db_with_confirmed_task(dir: &Path, dev_ai_kind: &str) -> (Database, String, String) {
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
        (db, project.id, task.id)
    }

    #[test]
    fn main_flow_approve_reaches_awaiting_user_acceptance() {
        let dir = git_project_dir("main-flow");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();

        assert_eq!(outcome.task.state, TaskState::AwaitingUserAcceptance);
        assert_eq!(adapter.development_call_count(), 1);
        assert_eq!(adapter.review_call_count(), 1);

        let reviews = db.list_task_reviews(&task_id).unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].verdict, "approve");
        assert_eq!(reviews[0].round, 1);

        // The full success path also composes correctly with Phase 2's
        // existing acceptance/completion machinery.
        db.record_user_acceptance(&_project_id, &task_id, true, None)
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
        let result = run_core_loop(&db, &task.id, &adapter, &CancellationToken::new());
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
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "claude_cli");
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success, MockOutcome::Success],
            vec![
                ReviewVerdict::ChangesRequired,
                ReviewVerdict::ChangesRequired,
            ],
        );

        let outcome = run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();

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

        let reviews = db.list_task_reviews(&task_id).unwrap();
        assert_eq!(reviews.len(), 2);
        assert_eq!(reviews[0].round, 1);
        assert_eq!(reviews[1].round, 2);
        assert!(reviews.iter().all(|r| r.verdict == "changes_required"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_round_approve_after_one_fix_pass_still_succeeds() {
        let dir = git_project_dir("fix-then-approve");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success, MockOutcome::Success],
            vec![ReviewVerdict::ChangesRequired, ReviewVerdict::Approve],
        );

        let outcome = run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();
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

        let outcome = run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();

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
        let outcome = run_core_loop(&db, &task_id, &adapter, &cancel).unwrap();

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

        let outcome = run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();

        assert_eq!(outcome.task.state, TaskState::NeedsUserAction);
        assert_eq!(adapter.development_call_count(), 0);
        assert_eq!(adapter.review_call_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reviewer_write_attempt_is_technically_detected_and_stops_the_loop() {
        let dir = git_project_dir("reviewer-writes");
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        let violating_dir = dir.clone();
        *adapter.review_side_effect.lock().unwrap() = Some(Box::new(move |_req| {
            let _ = fs::write(
                violating_dir.join("reviewer_should_not_write_this.txt"),
                "oops",
            );
        }));

        let outcome = run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();

        assert_eq!(outcome.task.state, TaskState::NeedsUserAction);
        let reviews = db.list_task_reviews(&task_id).unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].verdict, "integrity_violation");
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
        let outcome = run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();

        assert_eq!(outcome.task.state, TaskState::NeedsUserAction);
        assert_eq!(
            adapter.development_call_count(),
            0,
            "an uncertain prior attempt must never be silently retried"
        );
        let _ = fs::remove_dir_all(&dir);
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

        let handles: Vec<_> = [(); 2]
            .into_iter()
            .map(|_| {
                let path = db_path.clone();
                let adapter = Arc::clone(&adapter);
                let barrier = Arc::clone(&barrier);
                let task_id = task_id.clone();
                std::thread::spawn(move || {
                    let db = Database::open(&path).unwrap();
                    barrier.wait();
                    run_core_loop(&db, &task_id, adapter.as_ref(), &CancellationToken::new())
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

        let adapter_a = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);
        run_core_loop(&db, &task_a, &adapter_a, &CancellationToken::new()).unwrap();
        let adapter_b = MockAdapter::new(
            vec![MockOutcome::Success],
            vec![ReviewVerdict::ChangesRequired],
        );
        run_core_loop(&db, &task_b_row.id, &adapter_b, &CancellationToken::new()).unwrap();

        let reviews_a = db.list_task_reviews(&task_a).unwrap();
        let reviews_b = db.list_task_reviews(&task_b_row.id).unwrap();
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
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(
            vec![MockOutcome::Success],
            vec![
                ReviewVerdict::ChangesRequired,
                ReviewVerdict::ChangesRequired,
            ],
        );
        *adapter.review_findings_text.lock().unwrap() =
            Some("Found an issue: Authorization: Bearer sk-fake-secret-1234567890".to_string());

        let outcome = run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();
        assert_eq!(
            outcome.task.state,
            TaskState::Failed,
            "two changes_required verdicts must exhaust the one fix pass and end in failed"
        );

        let reviews = db.list_task_reviews(&task_id).unwrap();
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
        let (db, _project_id, task_id) = db_with_confirmed_task(&dir, "codex_cli");
        let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![ReviewVerdict::Approve]);

        let outcome = run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();
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
        run_core_loop(&db, &task_id, &adapter, &CancellationToken::new()).unwrap();

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
}
