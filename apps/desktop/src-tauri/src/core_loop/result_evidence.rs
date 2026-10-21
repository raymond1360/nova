//! Read-only attribution of the evidence adopted by the current result.
//! Never rebinds records or searches another round after a selected pair fails.
use super::*;
use crate::models::{BuildEvidence, TestEvidence};

pub struct AdoptedEvidence {
    pub generation: i64,
    pub round: i64,
    pub dev_attempt: i64,
    pub test: TestEvidence,
    pub build: BuildEvidence,
}

pub fn adopted(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
) -> NovaResult<Option<AdoptedEvidence>> {
    let stored = db.task(task_id)?;
    if stored.project_id != project_id
        || task.id != task_id
        || task.project_id != project_id
        || stored.result_generation != task.result_generation
    {
        return Err(NovaError::Config("结果证据归属不匹配".into()));
    }
    if crate::storage::tasks::is_legacy_frozen(task) {
        return Ok(None);
    }
    // Preserve the backend's fixed candidate selection; validation cannot
    // rescue a rejected attempt by choosing a different historical success.
    let Some((generation, round)) =
        documents::candidate_passing_generation_round(db, project_id, task_id, task)?
    else {
        return Ok(None);
    };
    if generation != task.result_generation {
        if !matches!(
            task.state,
            TaskState::PreviewReady | TaskState::AwaitingUserAcceptance | TaskState::Completed
        ) {
            return Ok(None);
        }
        // New observations supersede the pre-preview result, even if failed.
        for r in [1, 2] {
            if !db
                .list_test_evidence_for_round(project_id, task_id, task.result_generation, r)?
                .is_empty()
                || !db
                    .list_build_evidence_for_round(project_id, task_id, task.result_generation, r)?
                    .is_empty()
            {
                return Ok(None);
            }
        }
    }
    let Some(dev) = db.find_action(task_id, &format!("dev-{task_id}-round-{round}"))? else {
        return Ok(None);
    };
    if dev.status != ActionStatus::Succeeded {
        return Ok(None);
    }
    // NULL-attempt legacy/orphan rows are retained for audit, never adopted.
    let test = db
        .list_test_evidence_for_round(project_id, task_id, generation, round)?
        .into_iter()
        .filter(|e| e.result_attempt.is_some())
        .next_back();
    let build = db
        .list_build_evidence_for_round(project_id, task_id, generation, round)?
        .into_iter()
        .filter(|e| e.result_attempt.is_some())
        .next_back();
    let (Some(test), Some(build)) = (test, build) else {
        return Ok(None);
    };
    let attempt = Some(dev.attempt_number);
    if test.result_attempt != attempt || build.result_attempt != attempt {
        return Ok(None);
    }
    let test_ok = (test.status == "passed" && test.exit_code == Some(0) && test.failed_count == 0)
        || (test.status == "no_tests_applicable"
            && documents::no_tests_applicable_is_authorized(
                db, project_id, task_id, generation, round, attempt,
            )?);
    let build_ok = (build.status == "passed" && build.exit_code == Some(0))
        || (build.status == "no_build_applicable"
            && documents::no_build_applicable_is_authorized(
                db,
                project_id,
                task_id,
                generation,
                round,
                attempt,
                build.authorization_source.as_deref(),
            )?);
    if !test_ok || !build_ok {
        return Ok(None);
    }
    Ok(Some(AdoptedEvidence {
        generation,
        round,
        dev_attempt: dev.attempt_number,
        test,
        build,
    }))
}

/// An adopted row is current even though preview_ready advanced the result
/// generation. Other rows keep their literal execution generation and history.
pub fn adopted_test(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
    id: &str,
) -> NovaResult<bool> {
    Ok(adopted(db, project_id, task_id, task)?.is_some_and(|a| a.test.id == id))
}
pub fn adopted_build(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
    id: &str,
) -> NovaResult<bool> {
    Ok(adopted(db, project_id, task_id, task)?.is_some_and(|a| a.build.id == id))
}
