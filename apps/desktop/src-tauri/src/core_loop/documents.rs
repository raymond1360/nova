//! Phase 3 remediation (Major 5), reframed by the v0.1.0 scope correction
//! (Work Package 2, `CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`): the
//! seven documents Nova generates for a task, regenerated in full from the
//! database's *current* truth every time it is called — never
//! incrementally patched — so the on-disk documents can never drift out of
//! sync with (or contradict) a later state change. Every value written
//! comes from Nova's own independently-recorded evidence (the Action
//! Ledger, `task_reviews`, `test_evidence`, `user_acceptance`) — never from
//! anything an AI merely claimed. A document for a step that has not
//! happened yet still exists, with an explicit "not yet triggered" status,
//! rather than being silently absent.
//!
//! Of the seven, only four are v0.1.0's *required* result set:
//! `REQUIREMENT.md`, `DEVELOPMENT_REPORT.md`, `TEST_REPORT.md`, and
//! `FINAL_RESULT.md`. `FIRST_REVIEW.md`/`SECOND_REVIEW.md` (and, by
//! extension, `FIX_REPORT.md`'s relationship to a second review round) are
//! the dormant v0.2.0 dual-AI reviewer's output — kept here purely as
//! optional diagnostic material for backward/forward compatibility with any
//! `task_reviews` rows that do exist (including historical ones from
//! before this correction), never deleted, and still generated
//! unconditionally so existing tooling and older projects keep reading a
//! stable seven-file shape. [`final_result_doc`] is the actual v0.1.0
//! completion authority, and it never reads a review verdict to decide
//! anything — see its own doc comment.

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
        &review_doc(
            db,
            project_id,
            task_id,
            &task,
            1,
            "First review (optional diagnostic — v0.2.0 dual-AI reviewer, not required for v0.1.0)",
        )?,
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
            "Second review (optional diagnostic — v0.2.0 dual-AI reviewer, not required for v0.1.0)",
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

/// v0.1.0 scope correction (`CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`,
/// Blocker 4): completion is gated on Nova's own independent test/build
/// verification for the current result generation — never on a reviewer
/// `approve` row. The dormant v0.2.0 dual-AI path may still write real
/// `task_reviews` rows (kept as supplementary evidence, surfaced in
/// FIRST_REVIEW.md/SECOND_REVIEW.md when present), but their absence must
/// never block a v0.1.0 single-AI result from being shown as ready for user
/// acceptance. Checks both rounds (1 and 2, the fixed "at most one automatic
/// fix pass" product rule) since either could be the one that actually
/// produced the current result.
///
/// Real bug found via testing: `tasks.result_generation` is bumped on the
/// `-> preview_ready` transition, which happens *after* the round's test
/// evidence was already recorded against the *pre-bump* generation — so a
/// task that has just reached `awaiting_user_acceptance` for the first time
/// always has its real, passing test evidence one generation behind
/// `task.result_generation` as read here. Checking both
/// `task.result_generation` and `task.result_generation - 1` covers both
/// that case and the (harmless, already-correct) case of re-checking an
/// older, already-superseded generation.
///
/// Phase 4 (evidence/document gate): extracted unchanged from
/// `final_result_doc` (previously inlined there) so the completion
/// authority has exactly one implementation — `evidence_overview`'s
/// `acceptance_ready`/`is_completed` fields and `FINAL_RESULT.md`'s own
/// status line now both call this, instead of risking two independently
/// hand-written copies of the same gate silently drifting apart. Behavior
/// is unchanged: see `final_result_never_says_completed_before_a_real_user_
/// acceptance_exists` and `final_result_shows_test_and_build_evidence_
/// separately` below, which still assert on the exact same rendered output.
pub(crate) fn adopted_passing_generation_round(
    db: &Database,
    project_id: &str,
    task_id: &str,
    task: &Task,
) -> NovaResult<Option<(i64, i64)>> {
    for generation in [task.result_generation, task.result_generation - 1] {
        if generation < 0 {
            continue;
        }
        for round in [1_i64, 2] {
            let mut test_ok = false;
            for evidence in
                db.list_test_evidence_for_round(project_id, task_id, generation, round)?
            {
                if evidence.status == "passed" {
                    test_ok = true;
                    break;
                }
                if evidence.status == "no_tests_applicable"
                    && no_tests_applicable_is_authorized(
                        db, project_id, task_id, generation, round,
                    )?
                {
                    test_ok = true;
                    break;
                }
            }
            let mut build_ok = false;
            for evidence in
                db.list_build_evidence_for_round(project_id, task_id, generation, round)?
            {
                if evidence.status == "passed" {
                    build_ok = true;
                    break;
                }
                if evidence.status == "no_build_applicable"
                    && no_build_applicable_is_authorized(
                        db, project_id, task_id, generation, round,
                    )?
                {
                    build_ok = true;
                    break;
                }
            }
            if test_ok && build_ok {
                return Ok(Some((generation, round)));
            }
        }
    }
    Ok(None)
}

/// Phase 4 remediation (Major 2): `no_tests_applicable` evidence status
/// string alone is never authorization — it is legitimate only when a real
/// `no_tests_confirmations` row exists for this *exact*
/// `(project_id, task_id, result_generation, round)` natural key. Unlike
/// build (see [`no_build_applicable_is_authorized`]), test detection
/// (`test_runner::detect_test_command`) has no mechanical "positively
/// confirmed no tests apply" case — a missing test command is always either
/// a genuine detection gap awaiting confirmation, or nothing at all — so
/// this check alone is complete for tests, with no fallback needed.
pub(crate) fn no_tests_applicable_is_authorized(
    db: &Database,
    project_id: &str,
    task_id: &str,
    result_generation: i64,
    round: i64,
) -> NovaResult<bool> {
    Ok(db
        .find_no_tests_confirmation(project_id, task_id, result_generation, round)?
        .is_some())
}

/// Phase 4 remediation (Major 2): the build equivalent of
/// [`no_tests_applicable_is_authorized`], with one deliberate addition —
/// `no_build_applicable` has two distinct legitimate origins (see
/// `Database::record_build_evidence`'s own doc comment for the full
/// audit): a real `no_build_confirmations` row for this exact key, **or**
/// Nova's own current, mechanical re-derivation
/// (`test_runner::detect_build_command`) independently and positively
/// confirming this project's build step genuinely does not apply, exactly
/// mirroring the same check `core_loop::run_build_for_round` performs
/// before ever writing this evidence in the first place. Re-deriving
/// against the project's *current* files (rather than trusting the
/// evidence row alone) is strictly more conservative than the original
/// write-time decision: if the project has since gained a build script,
/// this correctly stops trusting a now-stale "not applicable" claim.
pub(crate) fn no_build_applicable_is_authorized(
    db: &Database,
    project_id: &str,
    task_id: &str,
    result_generation: i64,
    round: i64,
) -> NovaResult<bool> {
    if db
        .find_no_build_confirmation(project_id, task_id, result_generation, round)?
        .is_some()
    {
        return Ok(true);
    }
    let project = db.project(project_id)?;
    let project_dir = Path::new(&project.local_path);
    Ok(matches!(
        super::test_runner::detect_build_command(project_dir),
        super::test_runner::BuildDetection::NotApplicable(_)
    ))
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

    // Codex re-review remediation (second re-review, Minor): the status
    // line and the evidence breakdown below must always agree on *which*
    // generation actually satisfied the gate. Previously the gate check
    // could accept `task.result_generation - 1` (see the off-by-one note
    // on [`adopted_passing_generation_round`]) while the evidence breakdown
    // unconditionally rendered only `task.result_generation`, so a task
    // that had just reached `awaiting_user_acceptance` could show `status:
    // 等待用户验收` above a `(尚未触发)` evidence line — internally
    // contradictory. A single search now finds the *one* `(generation,
    // round)` pair whose test *and* build evidence both actually passed
    // together (the same pair `finish_success_path` itself required to let
    // that round succeed — test and build evidence for a round are always
    // written under the same `result_generation`, so this is never a
    // mismatched combination across rounds), and that exact pair drives
    // both the status line and the evidence breakdown below.
    let adopted = adopted_passing_generation_round(db, project_id, task_id, task)?;
    let gate_ok = adopted.is_some();

    let status_line = if task.state == TaskState::Completed && accepted_current_generation {
        "已完成（用户已验收）"
    } else if task.state == TaskState::AwaitingUserAcceptance && gate_ok {
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

    // Codex re-review remediation (Major 3 in the first re-review,
    // automated test requirement "FINAL_RESULT 能分别展示测试和构建证据"):
    // explicit, per-round evidence breakdown so a reader can tell "tests
    // passed but build failed" apart from "build passed but tests failed"
    // apart from "neither has run yet" — never collapsed into a single
    // pass/fail summary line. Displays the *adopted* generation when one
    // was found (the exact generation the status line above is reporting
    // on), never a mismatched one; falls back to the task's current
    // generation when nothing has been adopted yet (task still in
    // progress, or genuinely failed/cancelled).
    let display_generation = adopted
        .map(|(generation, _)| generation)
        .unwrap_or(task.result_generation);
    let mut evidence_breakdown = String::from("\n## 测试与构建证据\n\n");
    if let Some((adopted_generation, adopted_round)) = adopted {
        evidence_breakdown.push_str(&format!(
            "采用的有效证据：result_generation={adopted_generation}, round={adopted_round}"
        ));
        if adopted_generation != task.result_generation {
            evidence_breakdown.push_str(&format!(
                "（注意：当前 result_generation={} 尚无证据，采用的是上一代 {adopted_generation} 的证据）",
                task.result_generation
            ));
        }
        evidence_breakdown.push('\n');
    }
    for round in [1_i64, 2] {
        let test_status = db
            .list_test_evidence_for_round(project_id, task_id, display_generation, round)?
            .into_iter()
            .next_back()
            .map(|e| e.status)
            .unwrap_or_else(|| "(尚未触发)".to_string());
        let build_status = db
            .list_build_evidence_for_round(project_id, task_id, display_generation, round)?
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

    // --- Phase 4 remediation (Major 2): not_applicable requires an exact,
    // matching confirmation (or, build-only, a real mechanical
    // re-derivation) — never the bare evidence status string alone. ------

    use crate::storage::evidence::{RecordBuildEvidenceRequest, RecordTestEvidenceRequest};

    fn record_no_tests_applicable(
        db: &Database,
        project_id: &str,
        task_id: &str,
        generation: i64,
        round: i64,
    ) {
        db.record_test_evidence(&RecordTestEvidenceRequest {
            project_id,
            task_id,
            result_generation: generation,
            round,
            test_command_redacted: None,
            status: "no_tests_applicable",
            passed_count: 0,
            failed_count: 0,
            skipped_count: 0,
            exit_code: None,
            reason: Some("no test framework detected"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
    }

    /// Constructs an adversarial, invariant-breaking database state — a
    /// `no_tests_applicable` row with **no** matching confirmation — that
    /// the real production write path (`Database::record_test_evidence`)
    /// now correctly refuses to create. Deliberately bypasses that guard
    /// via raw SQL, exactly as authorized for testing "what if an orphaned
    /// row already exists" (e.g. from data created before this remediation,
    /// or any other means outside the normal write path) without
    /// duplicating or weakening the real write-time check itself.
    fn insert_orphaned_no_tests_applicable(
        db: &Database,
        project_id: &str,
        task_id: &str,
        generation: i64,
        round: i64,
    ) {
        db.conn()
            .execute(
                "INSERT INTO test_evidence (id, project_id, task_id, result_generation, round, test_command_redacted, status, passed_count, failed_count, skipped_count, exit_code, reason, stdout_ref, stderr_ref, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'no_tests_applicable', 0, 0, 0, NULL, 'orphaned adversarial row', NULL, NULL, ?6)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    project_id,
                    task_id,
                    generation,
                    round,
                    chrono::Utc::now().to_rfc3339(),
                ],
            )
            .unwrap();
    }

    fn record_no_build_applicable(
        db: &Database,
        project_id: &str,
        task_id: &str,
        generation: i64,
        round: i64,
    ) {
        db.record_build_evidence(&RecordBuildEvidenceRequest {
            project_id,
            task_id,
            result_generation: generation,
            round,
            build_command_redacted: None,
            status: "no_build_applicable",
            exit_code: None,
            reason: Some("no build step detected"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
    }

    fn record_test_passed(
        db: &Database,
        project_id: &str,
        task_id: &str,
        generation: i64,
        round: i64,
    ) {
        db.record_test_evidence(&RecordTestEvidenceRequest {
            project_id,
            task_id,
            result_generation: generation,
            round,
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
    }

    fn record_build_passed(
        db: &Database,
        project_id: &str,
        task_id: &str,
        generation: i64,
        round: i64,
    ) {
        db.record_build_evidence(&RecordBuildEvidenceRequest {
            project_id,
            task_id,
            result_generation: generation,
            round,
            build_command_redacted: Some("cargo build"),
            status: "passed",
            exit_code: Some(0),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
    }

    fn task_at_generation(db: &Database, task_id: &str, generation: i64) -> Task {
        let mut task = db.task(task_id).unwrap();
        task.result_generation = generation;
        task
    }

    #[test]
    fn applicable_evidence_with_matching_confirmation_is_adopted() {
        let dir = temp_dir("major2-happy-path");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, None)
            .unwrap();
        record_no_tests_applicable(&db, &project_id, &task_id, 0, 1);
        record_no_build_applicable(&db, &project_id, &task_id, 0, 1);

        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(adopted, Some((0, 1)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn applicable_test_evidence_without_any_confirmation_is_never_adopted() {
        let dir = temp_dir("major2-test-orphaned");
        let (db, project_id, task_id) = db_with_task(&dir);
        insert_orphaned_no_tests_applicable(&db, &project_id, &task_id, 0, 1);
        record_build_passed(&db, &project_id, &task_id, 0, 1);

        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(
            adopted, None,
            "an unconfirmed no_tests_applicable row must never authorize completion"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn applicable_build_evidence_without_confirmation_or_mechanical_justification_is_never_adopted()
    {
        // `db_with_task`'s fixture directory is genuinely empty (no
        // Cargo.toml, no package.json) — `detect_build_command` therefore
        // returns `Insufficient`, not `NotApplicable`, so the mechanical
        // fallback must not kick in here either.
        let dir = temp_dir("major2-build-orphaned");
        let (db, project_id, task_id) = db_with_task(&dir);
        record_test_passed(&db, &project_id, &task_id, 0, 1);
        record_no_build_applicable(&db, &project_id, &task_id, 0, 1);

        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(
            adopted, None,
            "an unconfirmed no_build_applicable row with no mechanical justification must never authorize completion"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mechanically_justified_no_build_applicable_is_adopted_without_any_confirmation() {
        // Protects the *legitimate* design this round must not break:
        // `BuildDetection::NotApplicable` (a real package.json with no
        // build script) never needs or produces a confirmation.
        let dir = temp_dir("major2-build-mechanical");
        let (db, project_id, task_id) = db_with_task(&dir);
        fs::write(dir.join("package.json"), r#"{"name":"x","scripts":{}}"#).unwrap();
        record_test_passed(&db, &project_id, &task_id, 0, 1);
        record_no_build_applicable(&db, &project_id, &task_id, 0, 1);

        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(
            adopted,
            Some((0, 1)),
            "a mechanically-detected NotApplicable build must still be adopted without a confirmation"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmation_for_a_stale_generation_never_authorizes_the_current_generations_evidence() {
        let dir = temp_dir("major2-stale-generation");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_tests_confirmation(&project_id, &task_id, 5, 1, None)
            .unwrap();
        insert_orphaned_no_tests_applicable(&db, &project_id, &task_id, 0, 1);
        record_build_passed(&db, &project_id, &task_id, 0, 1);

        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(adopted, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmation_for_a_stale_round_never_authorizes_the_current_rounds_evidence() {
        let dir = temp_dir("major2-stale-round");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 2, None)
            .unwrap();
        insert_orphaned_no_tests_applicable(&db, &project_id, &task_id, 0, 1);
        record_build_passed(&db, &project_id, &task_id, 0, 1);

        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(adopted, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmation_belonging_to_a_different_task_never_authorizes_this_tasks_evidence() {
        let dir = temp_dir("major2-wrong-task");
        let (db, project_id, task_id) = db_with_task(&dir);
        let other_task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project_id.clone(),
                    title: "Other".to_string(),
                    goal: None,
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: None,
                },
                Actor::User,
            )
            .unwrap();
        db.record_no_tests_confirmation(&project_id, &other_task.id, 0, 1, None)
            .unwrap();
        insert_orphaned_no_tests_applicable(&db, &project_id, &task_id, 0, 1);
        record_build_passed(&db, &project_id, &task_id, 0, 1);

        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(adopted, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_confirmation_belonging_to_a_different_project_never_authorizes_this_tasks_evidence() {
        let dir_a = temp_dir("major2-wrong-project-a");
        let dir_b = temp_dir("major2-wrong-project-b");
        let (db, project_a, task_a) = db_with_task(&dir_a);
        let (_db_unused, project_b, _task_b) = db_with_task(&dir_b);
        // A confirmation genuinely exists, just under a different project —
        // the natural key (including project_id) must not match task_a's.
        let cross_project_result = db.record_no_tests_confirmation(&project_b, &task_a, 0, 1, None);
        assert!(
            cross_project_result.is_err(),
            "storage itself already refuses to write a confirmation for a task the project doesn't own"
        );
        insert_orphaned_no_tests_applicable(&db, &project_a, &task_a, 0, 1);
        record_build_passed(&db, &project_a, &task_a, 0, 1);

        let task = task_at_generation(&db, &task_a, 0);
        let adopted = adopted_passing_generation_round(&db, &project_a, &task_a, &task).unwrap();
        assert_eq!(
            adopted, None,
            "no confirmation exists for task_a at all, so its no_tests_applicable evidence must not be adopted"
        );
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn test_and_build_evidence_from_different_generations_never_pair_up() {
        let dir = temp_dir("major2-mismatched-generation");
        let (db, project_id, task_id) = db_with_task(&dir);
        record_test_passed(&db, &project_id, &task_id, 0, 1);
        record_build_passed(&db, &project_id, &task_id, 1, 1);

        let task = task_at_generation(&db, &task_id, 1);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(
            adopted, None,
            "test passing at generation 0 and build passing at generation 1 must never combine"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_and_build_evidence_from_different_rounds_never_pair_up() {
        let dir = temp_dir("major2-mismatched-round");
        let (db, project_id, task_id) = db_with_task(&dir);
        record_test_passed(&db, &project_id, &task_id, 0, 1);
        record_build_passed(&db, &project_id, &task_id, 0, 2);

        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(
            adopted, None,
            "round 1 test evidence and round 2 build evidence must never combine"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legitimate_no_tests_and_no_build_confirmations_still_adopt_normally() {
        let dir = temp_dir("major2-both-legitimate");
        let (db, project_id, task_id) = db_with_task(&dir);
        db.record_no_tests_confirmation(&project_id, &task_id, 0, 1, Some("no framework"))
            .unwrap();
        db.record_no_build_confirmation(&project_id, &task_id, 0, 1, Some("no build needed"))
            .unwrap();
        record_no_tests_applicable(&db, &project_id, &task_id, 0, 1);
        record_no_build_applicable(&db, &project_id, &task_id, 0, 1);

        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(
            adopted,
            Some((0, 1)),
            "the ordinary, fully-legitimate no-tests + no-build product path must keep working"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn adopted_generation_round_facts_survive_a_real_file_backed_database_reopen() {
        let dir = temp_dir("major2-reopen");
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
            db.record_no_tests_confirmation(&project.id, &task.id, 0, 1, None)
                .unwrap();
            record_no_tests_applicable(&db, &project.id, &task.id, 0, 1);
            // Deliberately orphaned build evidence — no confirmation, and
            // this fixture dir has no Cargo.toml/package.json either.
            record_no_build_applicable(&db, &project.id, &task.id, 0, 1);
            (project.id, task.id)
        };

        let db = Database::open(&db_path).unwrap();
        let task = task_at_generation(&db, &task_id, 0);
        let adopted = adopted_passing_generation_round(&db, &project_id, &task_id, &task).unwrap();
        assert_eq!(
            adopted, None,
            "the legitimate no-tests confirmation must survive the reopen, but the orphaned \
             build evidence must still fail closed rather than becoming trusted after a restart"
        );
        let _ = fs::remove_dir_all(&dir);
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

    // --- Codex re-review remediation (second re-review, Minor): status
    // line and evidence breakdown must always agree on the adopted
    // generation ------------------------------------------------------

    fn drive_to_awaiting_user_acceptance_at_generation_zero(db: &Database, task_id: &str) {
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
                task_id,
                state,
                crate::models::Actor::System,
                None,
                None,
                false,
                None,
            )
            .unwrap();
        }
    }

    #[test]
    fn status_line_and_evidence_breakdown_agree_after_preview_ready_bumps_the_generation() {
        // The exact real bug this closes: evidence is written at
        // generation 0 (pre-bump), then `-> preview_ready` bumps
        // `tasks.result_generation` to 1 — the status line and the
        // evidence breakdown must both report generation 0's real evidence
        // consistently, never "等待用户验收" above a "(尚未触发)" line.
        let dir = temp_dir("final-result-gen-consistency");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("final-result-gen-consistency-appdata");

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
        drive_to_awaiting_user_acceptance_at_generation_zero(&db, &task_id);
        // The real `-> preview_ready` transition itself bumps
        // `tasks.result_generation` from 0 to 1 — one ahead of the
        // generation the evidence above was actually recorded under.
        let bumped_task = db.task(&task_id).unwrap();
        assert_eq!(
            bumped_task.result_generation, 1,
            "sanity check: the real preview_ready transition must have bumped the generation"
        );

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(content.contains("status: 等待用户验收"));
        assert!(
            !content.contains("Round 1: 测试=(尚未触发), 构建=(尚未触发)"),
            "status says awaiting acceptance but the evidence breakdown must not show \
             not-yet-triggered for the very round that satisfied the gate: {content}"
        );
        assert!(content.contains("Round 1: 测试=passed, 构建=passed"));
        assert!(
            content.contains("采用的是上一代 0 的证据"),
            "must explicitly note that generation 0 (not the current, post-bump generation 1) \
             is the one actually adopted: {content}"
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn evidence_older_than_generation_minus_one_is_never_adopted() {
        // Generation 0 has real passing evidence; the task's current
        // generation is bumped all the way to 2 (two full superseded
        // cycles). Generation 0 is now more than one behind — it must
        // never be silently reused; the status line must not claim
        // "等待用户验收" for evidence that stale.
        let dir = temp_dir("final-result-too-old-generation");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("final-result-too-old-generation-appdata");

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
        db.conn()
            .execute(
                "UPDATE tasks SET result_generation = 2 WHERE id = ?1",
                rusqlite::params![task_id],
            )
            .unwrap();
        drive_to_awaiting_user_acceptance_at_generation_zero(&db, &task_id);

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(
            !content.contains("status: 等待用户验收"),
            "generation-0 evidence is two generations stale relative to the current generation \
             2 and must never be adopted: {content}"
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn round_2_success_after_a_round_1_failure_displays_round_2_as_adopted() {
        let dir = temp_dir("final-result-round-2-adopted");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("final-result-round-2-adopted-appdata");

        db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
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
        db.record_build_evidence(&crate::storage::evidence::RecordBuildEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 2,
            build_command_redacted: Some("cargo build"),
            status: "passed",
            exit_code: Some(0),
            reason: None,
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        drive_to_awaiting_user_acceptance_at_generation_zero(&db, &task_id);

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(content.contains("status: 等待用户验收"));
        assert!(content.contains("round=2"));
        assert!(content.contains("Round 2: 测试=passed, 构建=passed"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn completed_state_still_shows_the_same_adopted_evidence_as_awaiting_acceptance_did() {
        let dir = temp_dir("final-result-completed-consistent");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("final-result-completed-consistent-appdata");

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
        drive_to_awaiting_user_acceptance_at_generation_zero(&db, &task_id);
        db.record_user_acceptance(&project_id, &task_id, true, Some("looks good"))
            .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Completed,
            crate::models::Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(content.contains("status: 已完成（用户已验收）"));
        assert!(content.contains("Round 1: 测试=passed, 构建=passed"));
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

    // --- v0.1.0 scope correction (Work Package 2): FINAL_RESULT must never
    // depend on FIRST_REVIEW/SECOND_REVIEW or any `task_reviews` verdict —
    // those remain optional diagnostic material only. --------------------

    #[test]
    fn final_result_reaches_awaiting_acceptance_and_completed_with_zero_task_reviews_ever_recorded()
    {
        // The real v0.1.0 single-AI path: no reviewer is ever invoked, so
        // `task_reviews` never receives a single row for this task. Both
        // `awaiting_user_acceptance` and, after a real user acceptance,
        // `completed` must still be reported correctly by FINAL_RESULT —
        // and FIRST_REVIEW.md/SECOND_REVIEW.md must still just say
        // "not yet triggered" rather than blocking anything.
        let dir = temp_dir("final-result-zero-reviews");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("final-result-zero-reviews-appdata");

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
        drive_to_awaiting_user_acceptance_at_generation_zero(&db, &task_id);
        assert!(
            db.list_task_reviews(&project_id, &task_id)
                .unwrap()
                .is_empty(),
            "sanity check: this task must genuinely have zero task_reviews rows"
        );

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let final_result = fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(final_result.contains("status: 等待用户验收"));
        let first_review = fs::read_to_string(doc_dir.join("FIRST_REVIEW.md")).unwrap();
        assert!(first_review.contains(NOT_TRIGGERED_ZH));
        let second_review = fs::read_to_string(doc_dir.join("SECOND_REVIEW.md")).unwrap();
        assert!(second_review.contains(NOT_TRIGGERED_ZH));

        db.record_user_acceptance(&project_id, &task_id, true, Some("looks good"))
            .unwrap();
        db.transition_task(
            &task_id,
            TaskState::Completed,
            crate::models::Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        let final_result_after_completion =
            fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(final_result_after_completion.contains("status: 已完成（用户已验收）"));
        assert!(
            db.list_task_reviews(&project_id, &task_id)
                .unwrap()
                .is_empty(),
            "must reach completed without ever recording a single review"
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn final_result_ignores_a_negative_historical_review_verdict() {
        // A `changes_required` review row on file (e.g. from an earlier
        // dormant v0.2.0 dual-AI attempt, or simply historical data) must
        // never block v0.1.0's own independent test/build/acceptance gate
        // — FINAL_RESULT is not a function of any review verdict, positive
        // or negative.
        let dir = temp_dir("final-result-negative-historical-review");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("final-result-negative-historical-review-appdata");

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
        db.record_task_review(
            &project_id,
            &task_id,
            0,
            1,
            "claude_cli",
            "changes_required",
            Some("historical negative verdict, must not gate v0.1.0"),
        )
        .unwrap();
        drive_to_awaiting_user_acceptance_at_generation_zero(&db, &task_id);

        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        let doc_dir = app_data_dir
            .join("result-documents")
            .join(&project_id)
            .join(&task_id);
        let content = fs::read_to_string(doc_dir.join("FINAL_RESULT.md")).unwrap();
        assert!(
            content.contains("status: 等待用户验收"),
            "a negative historical review verdict must never block the v0.1.0 test/build gate: {content}"
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }
}
