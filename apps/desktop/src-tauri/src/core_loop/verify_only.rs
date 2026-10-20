//! Explicit verification recovery. No AI adapter is accepted or constructed.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Eligibility {
    pub result_generation: i64,
    pub round: i64,
    pub dev_attempt: i64,
    pub test_command: String,
}

fn reject() -> NovaError {
    NovaError::Config(
        "当前任务不满足仅重新验证条件：需要当前代际/轮次的成功开发、自查完成及可恢复验证记录"
            .into(),
    )
}

pub fn eligibility(db: &Database, project_id: &str, task_id: &str) -> NovaResult<Eligibility> {
    let task = db.task(task_id)?;
    if task.project_id != project_id
        || task.state != TaskState::NeedsUserAction
        || crate::storage::tasks::is_legacy_frozen(&task)
    {
        return Err(reject());
    }
    let round = current_pending_round(db, project_id, task_id)?;
    let dev = db
        .find_action(task_id, &format!("dev-{task_id}-round-{round}"))?
        .ok_or_else(reject)?;
    if dev.status != ActionStatus::Succeeded {
        return Err(reject());
    }
    let check = db
        .current_self_check(task_id, round, dev.attempt_number)?
        .ok_or_else(reject)?;
    if check.parse_status != "valid"
        || check.self_reported_status.as_deref() != Some("self_reported_complete")
    {
        return Err(reject());
    }
    for items in [&check.unfinished_items_json, &check.blocking_issues_json] {
        let items: Vec<String> = items
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .ok_or_else(reject)?;
        if !items.is_empty() {
            return Err(reject());
        }
    }
    let expected: std::collections::BTreeSet<_> = db
        .list_confirmed_requirement_items(task_id)?
        .iter()
        .map(|i| format!("{}:{}", i.field, i.item_index))
        .collect();
    let entries: Vec<serde_json::Value> = check
        .requirement_coverage_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or_else(reject)?;
    let mut seen = std::collections::BTreeSet::new();
    for entry in entries {
        let key = entry["item"].as_str().ok_or_else(reject)?;
        if entry["status"] != "covered" || !expected.contains(key) || !seen.insert(key.to_string())
        {
            return Err(reject());
        }
    }
    if expected.is_empty() || seen != expected {
        return Err(reject());
    }
    let generation = task.result_generation;
    let mut failed = false;
    for key in [
        test_action_key(task_id, generation, round),
        build_action_key(task_id, generation, round),
    ] {
        if let Some(action) = db.find_action(task_id, &key)? {
            match action.status {
                ActionStatus::Failed => failed = true,
                ActionStatus::Succeeded | ActionStatus::Planned => {}
                _ => return Err(reject()), // Never acknowledge uncertain or live attempts.
            }
        }
    }
    if !failed {
        return Err(reject());
    }
    let command = task
        .test_command_redacted
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(reject)?;
    Ok(Eligibility {
        result_generation: generation,
        round,
        dev_attempt: dev.attempt_number,
        test_command: command,
    })
}

pub fn run(
    db: &Database,
    project_id: &str,
    task_id: &str,
    expected: &Eligibility,
    cancel: &CancellationToken,
    app_data_dir: &Path,
) -> NovaResult<CoreLoopOutcome> {
    // Revalidate the exact UI-reviewed identity; never trust caller round/attempt.
    if eligibility(db, project_id, task_id)? != *expected {
        return Err(reject());
    }
    let key = format!(
        "verify-only-{task_id}-g{}-r{}-a{}",
        expected.result_generation, expected.round, expected.dev_attempt
    );
    db.reset_failed_action_for_retry(task_id, &key)?;
    let action = match gate_action(db, task_id, "verification_only", &key, false)? {
        ActionGate::Started(a) => a,
        _ => {
            return Err(NovaError::Config(
                "仅验证已在执行或结果不确定，请勿重复操作".into(),
            ))
        }
    };
    let result = report_run_failure(db, task_id, || {
        // A competing request cannot pass this operation's Action Ledger CAS.
        if eligibility(db, project_id, task_id)? != *expected {
            return Err(reject());
        }
        for state in [TaskState::Queued, TaskState::Running, TaskState::Verifying] {
            db.transition_task(
                task_id,
                state,
                Actor::User,
                Some("仅重新验证现有修改；不调用 AI、不自动返修"),
                None,
                true,
                None,
            )?;
        }
        let dir = PathBuf::from(db.project(project_id)?.local_path);
        ensure_identity(db, project_id, task_id, expected)?;
        db.reset_failed_action_for_retry(
            task_id,
            &test_action_key(task_id, expected.result_generation, expected.round),
        )?;
        db.reset_failed_action_for_retry(
            task_id,
            &build_action_key(task_id, expected.result_generation, expected.round),
        )?;
        collect_git_evidence(db, project_id, task_id, &dir, &CancellationToken::new())?;
        match run_tests_for_round(
            db,
            project_id,
            task_id,
            &dir,
            expected.round,
            expected.dev_attempt,
            cancel,
        )? {
            TestRoundOutcome::Passed(_) | TestRoundOutcome::NoTestsApplicable(_) => {}
            TestRoundOutcome::Failed(reason) => return stop(db, task_id, &reason),
            _ => {
                return Ok(CoreLoopOutcome {
                    task: db.task(task_id)?,
                })
            }
        }
        ensure_identity(db, project_id, task_id, expected)?;
        match run_build_for_round(
            db,
            project_id,
            task_id,
            &dir,
            expected.round,
            expected.dev_attempt,
            cancel,
        )? {
            BuildRoundOutcome::Passed(_) | BuildRoundOutcome::NoBuildApplicable(_) => {}
            BuildRoundOutcome::Failed(reason) => return stop(db, task_id, &reason),
            _ => {
                return Ok(CoreLoopOutcome {
                    task: db.task(task_id)?,
                })
            }
        }
        ensure_identity(db, project_id, task_id, expected)?;
        finish_success_path(db, project_id, task_id, expected.round)?;
        Ok(CoreLoopOutcome {
            task: db.task(task_id)?,
        })
    });
    let success = result
        .as_ref()
        .map(|r| r.task.state == TaskState::AwaitingUserAcceptance)
        .unwrap_or(false);
    db.finish_action(
        task_id,
        &key,
        if success {
            ActionStatus::Succeeded
        } else {
            ActionStatus::Failed
        },
        None,
        if success {
            None
        } else {
            Some("仅验证已停止，未调用 AI 或自动返修")
        },
        action.attempt_number,
    )?;
    if let Err(error) = documents::generate_result_documents(db, project_id, task_id, app_data_dir)
    {
        let _ = db.set_document_sync_pending(task_id, true);
        return Err(error);
    }
    result
}

fn ensure_identity(
    db: &Database,
    project_id: &str,
    task_id: &str,
    expected: &Eligibility,
) -> NovaResult<()> {
    let task = db.task(task_id)?;
    let dev = db
        .find_action(task_id, &format!("dev-{task_id}-round-{}", expected.round))?
        .ok_or_else(reject)?;
    if task.project_id != project_id
        || task.state != TaskState::Verifying
        || task.result_generation != expected.result_generation
        || current_pending_round(db, project_id, task_id)? != expected.round
        || dev.status != ActionStatus::Succeeded
        || dev.attempt_number != expected.dev_attempt
        || task.test_command_redacted.as_deref() != Some(expected.test_command.as_str())
    {
        return Err(reject());
    }
    Ok(())
}

fn stop(db: &Database, task_id: &str, reason: &str) -> NovaResult<CoreLoopOutcome> {
    let reason = redact_sensitive_text(&format!(
        "仅验证未通过，已停止；不会调用 AI 或自动返修：{reason}"
    ));
    db.transition_task(
        task_id,
        TaskState::NeedsUserAction,
        Actor::System,
        Some(&reason),
        None,
        false,
        Some(&reason),
    )?;
    Ok(CoreLoopOutcome {
        task: db.task(task_id)?,
    })
}
