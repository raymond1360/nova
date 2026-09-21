//! Phase 3 remediation (Major 5): the seven-document set Nova generates
//! for a task, regenerated in full from the database's *current* truth
//! every time it is called — never incrementally patched — so the on-disk
//! documents can never drift out of sync with (or contradict) a later state
//! change. Every value written comes from Nova's own independently-recorded
//! evidence (the Action Ledger, `task_reviews`, `test_evidence`,
//! `user_acceptance`) — never from anything an AI merely claimed. A
//! document for a step that has not happened yet still exists, with an
//! explicit "not yet triggered" status, rather than being silently absent.

use std::path::{Path, PathBuf};

use crate::app_paths;
use crate::db::Database;
use crate::error::NovaResult;
use crate::models::{Task, TaskState};

const NOT_TRIGGERED_ZH: &str = "（尚未触发）";

/// Regenerates all seven documents for `task_id` under
/// `<app_data_dir>/result-documents/<project_id>/<task_id>/` — a third
/// app-owned root, distinct from `task-packages/` and `review-workspaces/`.
/// Safe to call at any point in the task's lifecycle; always reflects
/// exactly what the database currently says, nothing more.
pub fn generate_result_documents(
    db: &Database,
    project_id: &str,
    task_id: &str,
    app_data_dir: &Path,
) -> NovaResult<()> {
    let task = db.task(task_id)?;
    let dir = app_paths::safe_app_owned_dir(
        &app_data_dir.join("result-documents"),
        &[project_id, task_id],
    )
    .map_err(crate::error::NovaError::from)?;

    write_doc(&dir, "REQUIREMENT.md", &requirement_doc(&task))?;
    write_doc(
        &dir,
        "DEVELOPMENT_REPORT.md",
        &development_report_doc(db, task_id, 1)?,
    )?;
    write_doc(
        &dir,
        "TEST_REPORT.md",
        &test_report_doc(db, project_id, task_id, &task)?,
    )?;
    write_doc(
        &dir,
        "FIRST_REVIEW.md",
        &review_doc(db, project_id, task_id, &task, 1, "First review")?,
    )?;
    write_doc(&dir, "FIX_REPORT.md", &fix_report_doc(db, task_id)?)?;
    write_doc(
        &dir,
        "SECOND_REVIEW.md",
        &review_doc(
            db,
            project_id,
            task_id,
            &task,
            2,
            "Second review (automatic fix pass)",
        )?,
    )?;
    write_doc(
        &dir,
        "FINAL_RESULT.md",
        &final_result_doc(db, project_id, task_id, &task)?,
    )?;

    Ok(())
}

fn write_doc(dir: &Path, name: &str, content: &str) -> NovaResult<()> {
    let path: PathBuf = dir.join(name);
    app_paths::atomic_write(&path, content.as_bytes()).map_err(crate::error::NovaError::from)
}

fn requirement_doc(task: &Task) -> String {
    format!(
        "# REQUIREMENT\n\ntask_id: {}\nresult_generation: {}\n\n## Goal\n\n{}\n\n## In scope\n\n{}\n\n## Not in scope\n\n{}\n\n## Done means\n\n{}\n",
        task.id,
        task.result_generation,
        task.goal.as_deref().unwrap_or("(not specified)"),
        task.in_scope.as_deref().unwrap_or("(not specified)"),
        task.not_in_scope.as_deref().unwrap_or("(not specified)"),
        task.done_criteria.as_deref().unwrap_or("(not specified)"),
    )
}

fn development_report_doc(db: &Database, task_id: &str, round: i64) -> NovaResult<String> {
    let dev_key = format!("dev-{task_id}-round-{round}");
    let action = db.find_action(task_id, &dev_key)?;
    let body = match action {
        None => NOT_TRIGGERED_ZH.to_string(),
        Some(action) => format!(
            "status: {:?}\nstarted_at: {}\nfinished_at: {}\nerror_summary: {}\n",
            action.status,
            action.started_at.as_deref().unwrap_or("-"),
            action.finished_at.as_deref().unwrap_or("-"),
            action.error_summary.as_deref().unwrap_or("(none)"),
        ),
    };
    Ok(format!("# DEVELOPMENT_REPORT (round {round})\n\n{body}"))
}

fn test_report_doc(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
) -> NovaResult<String> {
    let mut body = String::from("# TEST_REPORT\n\n");
    for round in [1_i64, 2] {
        let evidence =
            db.list_test_evidence_for_round(project_id, task_id, task.result_generation, round)?;
        body.push_str(&format!("## Round {round}\n\n"));
        if evidence.is_empty() {
            body.push_str(NOT_TRIGGERED_ZH);
            body.push_str("\n\n");
            continue;
        }
        for row in &evidence {
            body.push_str(&format!(
                "- status: {}\n  command: {}\n  passed: {}, failed: {}, skipped: {}\n  reason: {}\n\n",
                row.status,
                row.test_command_redacted.as_deref().unwrap_or("(none)"),
                row.passed_count,
                row.failed_count,
                row.skipped_count,
                row.reason.as_deref().unwrap_or("-"),
            ));
        }
    }
    Ok(body)
}

fn review_doc(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
    round: i64,
    title: &str,
) -> NovaResult<String> {
    let reviews = db.list_task_reviews(project_id, task_id)?;
    let this_round: Vec<_> = reviews
        .iter()
        .filter(|r| r.round == round && r.result_generation == task.result_generation)
        .collect();
    let mut body = format!("# {title}\n\n");
    if this_round.is_empty() {
        body.push_str(NOT_TRIGGERED_ZH);
        body.push('\n');
        return Ok(body);
    }
    for review in this_round {
        body.push_str(&format!(
            "reviewer: {}\nverdict: {}\nfindings:\n\n{}\n",
            review.reviewer_ai_kind,
            review.verdict,
            review.findings_summary.as_deref().unwrap_or("(none)"),
        ));
    }
    Ok(body)
}

fn fix_report_doc(db: &Database, task_id: &str) -> NovaResult<String> {
    let dev_key = format!("dev-{task_id}-round-2");
    let action = db.find_action(task_id, &dev_key)?;
    let body = match action {
        None => format!("# FIX_REPORT\n\n{NOT_TRIGGERED_ZH}\n"),
        Some(action) => format!(
            "# FIX_REPORT\n\nstatus: {:?}\nstarted_at: {}\nfinished_at: {}\n",
            action.status,
            action.started_at.as_deref().unwrap_or("-"),
            action.finished_at.as_deref().unwrap_or("-"),
        ),
    };
    Ok(body)
}

fn final_result_doc(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
) -> NovaResult<String> {
    let acceptance = db.user_acceptance_for_task(task_id)?;
    let accepted_current_generation = acceptance
        .as_ref()
        .is_some_and(|a| a.result_generation == task.result_generation && a.accepted);

    // v0.1.0 scope correction (`CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`,
    // Blocker 4): completion is gated on Nova's own independent test/build
    // verification for the current result generation — never on a reviewer
    // `approve` row. The dormant v0.2.0 dual-AI path may still write real
    // `task_reviews` rows (kept as supplementary evidence, surfaced in
    // FIRST_REVIEW.md/SECOND_REVIEW.md below when present), but their
    // absence must never block a v0.1.0 single-AI result from being shown
    // as ready for user acceptance. Checks both rounds (1 and 2, the fixed
    // "at most one automatic fix pass" product rule) since either could be
    // the one that actually produced the current result.
    //
    // Real bug found via testing: `tasks.result_generation` is bumped on
    // the `-> preview_ready` transition, which happens *after* the round's
    // test evidence was already recorded against the *pre-bump* generation
    // — so a task that has just reached `awaiting_user_acceptance` for the
    // first time always has its real, passing test evidence one generation
    // behind `task.result_generation` as read here. Checking both
    // `task.result_generation` and `task.result_generation - 1` covers
    // both that case and the (harmless, already-correct) case of
    // re-checking an older, already-superseded generation.
    let test_gate_ok = {
        let mut ok = false;
        'generations: for generation in [task.result_generation, task.result_generation - 1] {
            if generation < 0 {
                continue;
            }
            for round in [1_i64, 2] {
                let evidence =
                    db.list_test_evidence_for_round(project_id, task_id, generation, round)?;
                if evidence
                    .iter()
                    .any(|e| e.status == "passed" || e.status == "no_tests_applicable")
                {
                    ok = true;
                    break 'generations;
                }
            }
        }
        ok
    };

    // Codex re-review remediation (Major 3): a symmetric build gate
    // alongside the test gate above — the same generation-off-by-one
    // handling applies (build evidence for a round is written before that
    // round's `-> preview_ready` transition bumps `result_generation`).
    let build_gate_ok = {
        let mut ok = false;
        'generations: for generation in [task.result_generation, task.result_generation - 1] {
            if generation < 0 {
                continue;
            }
            for round in [1_i64, 2] {
                let evidence =
                    db.list_build_evidence_for_round(project_id, task_id, generation, round)?;
                if evidence
                    .iter()
                    .any(|e| e.status == "passed" || e.status == "no_build_applicable")
                {
                    ok = true;
                    break 'generations;
                }
            }
        }
        ok
    };

    let status_line = if task.state == TaskState::Completed && accepted_current_generation {
        "已完成（用户已验收）"
    } else if task.state == TaskState::AwaitingUserAcceptance && test_gate_ok && build_gate_ok {
        "等待用户验收"
    } else if task.state == TaskState::Failed {
        "未通过：Nova 独立测试/构建验证未通过（已尝试一次修复）"
    } else if task.state == TaskState::Cancelled {
        "已取消"
    } else if task.state == TaskState::NeedsUserAction {
        "需要用户处理"
    } else {
        "进行中"
    };

    // Codex re-review remediation (Major 3, automated test requirement
    // "FINAL_RESULT 能分别展示测试和构建证据"): explicit, per-round evidence
    // breakdown so a reader can tell "tests passed but build failed" apart
    // from "build passed but tests failed" apart from "neither has run
    // yet" — never collapsed into a single pass/fail summary line.
    let mut evidence_breakdown = String::from("\n## 测试与构建证据\n\n");
    for round in [1_i64, 2] {
        let test_status = db
            .list_test_evidence_for_round(project_id, task_id, task.result_generation, round)?
            .into_iter()
            .next_back()
            .map(|e| e.status)
            .unwrap_or_else(|| "(尚未触发)".to_string());
        let build_status = db
            .list_build_evidence_for_round(project_id, task_id, task.result_generation, round)?
            .into_iter()
            .next_back()
            .map(|e| e.status)
            .unwrap_or_else(|| "(尚未触发)".to_string());
        evidence_breakdown.push_str(&format!(
            "- Round {round}: 测试={test_status}, 构建={build_status}\n"
        ));
    }

    Ok(format!(
        "# FINAL_RESULT\n\ntask_id: {}\nresult_generation: {}\ncurrent_state: {}\nstatus: {status_line}\n{evidence_breakdown}",
        task.id, task.result_generation, task.state,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Actor, CreateProjectRequest, CreateTaskRequest};
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-documents-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db_with_task(dir: &Path) -> (Database, String, String) {
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
                    goal: Some("goal text".to_string()),
                    in_scope: Some("in scope text".to_string()),
                    not_in_scope: None,
                    done_criteria: Some("done means text".to_string()),
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        (db, project.id, task.id)
    }

    #[test]
    fn generates_all_seven_documents_with_not_triggered_status_for_a_fresh_task() {
        let dir = temp_dir("fresh-task");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("fresh-task-appdata");

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        for name in [
            "REQUIREMENT.md",
            "DEVELOPMENT_REPORT.md",
            "TEST_REPORT.md",
            "FIRST_REVIEW.md",
            "FIX_REPORT.md",
            "SECOND_REVIEW.md",
            "FINAL_RESULT.md",
        ] {
            let content = fs::read_to_string(doc_dir.join(name))
                .unwrap_or_else(|e| panic!("{name} missing: {e}"));
            assert!(!content.trim().is_empty(), "{name} must not be empty");
        }
        let fix_report = fs::read_to_string(doc_dir.join("FIX_REPORT.md")).unwrap();
        assert!(fix_report.contains(NOT_TRIGGERED_ZH));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn requirement_doc_reflects_the_real_task_fields() {
        let dir = temp_dir("requirement-fields");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("requirement-fields-appdata");

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("REQUIREMENT.md")).unwrap();
        assert!(content.contains("goal text"));
        assert!(content.contains("in scope text"));
        assert!(content.contains("done means text"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn final_result_never_says_completed_before_a_real_user_acceptance_exists() {
        let dir = temp_dir("final-result-honest");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("final-result-honest-appdata");

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(!content.contains("已完成"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn documents_live_outside_the_project_directory_under_the_app_owned_root() {
        let dir = temp_dir("outside-project");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("outside-project-appdata");

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        assert!(!dir.join("result-documents").exists());
        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        assert!(doc_dir.join("REQUIREMENT.md").exists());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn final_result_shows_test_and_build_evidence_separately() {
        // Codex re-review remediation (Major 3): "tests passed but build
        // failed" must be readable as two distinct facts, never collapsed
        // into one pass/fail summary.
        let dir = temp_dir("final-result-test-vs-build");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("final-result-test-vs-build-appdata");

        db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            test_command_redacted: Some("cargo test"),
            status: "passed",
            passed_count: 3,
            failed_count: 0,
            skipped_count: 0,
            exit_code: Some(0),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            build_command_redacted: Some("cargo build"),
            status: "failed",
            exit_code: Some(1),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(content.contains("Round 1: 测试=passed, 构建=failed"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn final_result_awaiting_acceptance_requires_both_test_and_build_gates() {
        // A task sitting at `awaiting_user_acceptance` with passing test
        // evidence but no build evidence at all must not be reported as
        // "等待用户验收" — the build gate is symmetric with the test gate.
        let dir = temp_dir("final-result-build-gate");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("final-result-build-gate-appdata");

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
                &task_id,
                state,
                crate::models::Actor::System,
                None,
                None,
                false,
                None,
            )
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

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(
            !content.contains("status: 等待用户验收"),
            "must not report awaiting-acceptance without independent build evidence too: {content}"
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn regenerating_after_a_state_change_never_leaves_stale_conflicting_content() {
        let dir = temp_dir("regenerate-fresh");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("regenerate-fresh-appdata");

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        db.record_task_review(
            &project_id,
            &task_id,
            0,
            1,
            "claude_cli",
            "approve",
            Some("looks good"),
        )
        .unwrap();
        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("FIRST_REVIEW.md")).unwrap();
        assert!(content.contains("approve"));
        assert!(!content.contains(NOT_TRIGGERED_ZH));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }
}
