//! Regression cases from CODEX_WP2A_IMPLEMENTATION_REVIEW10151053.
//! Every database and project is disposable; no real AI CLI is used.
use super::*;
use crate::models::{CreateProjectRequest, CreateTaskRequest};
use crate::runtime::ai_adapter::mock::{MockAdapter, MockOutcome};
use crate::storage::evidence::{RecordBuildEvidenceRequest, RecordTestEvidenceRequest};
use crate::storage::self_check::RecordSelfCheckRequest;
use std::sync::{Arc, Barrier};

struct Fixture {
    dir: PathBuf,
    db: Database,
    project: String,
    task: String,
}
impl Fixture {
    fn new(file: bool) -> Self {
        let dir = std::env::temp_dir().join(format!("nova-wp2a-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let db = if file {
            Database::open(&dir.join("test.sqlite3")).unwrap()
        } else {
            Database::memory().unwrap()
        };
        db.migrate().unwrap();
        let p = db
            .create_project(&CreateProjectRequest {
                name: "WP2A".into(),
                local_path: dir.to_string_lossy().into_owned(),
                source_type: "local".into(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap();
        let t = db
            .create_task(
                &CreateTaskRequest {
                    project_id: p.id.clone(),
                    title: "WP2A".into(),
                    goal: Some("export\nexport".into()),
                    in_scope: Some("CSV".into()),
                    not_in_scope: Some("cloud".into()),
                    done_criteria: Some("UTF8".into()),
                    ai_kind: Some("codex_cli".into()),
                },
                Actor::User,
            )
            .unwrap();
        let f = Self {
            dir,
            db,
            project: p.id,
            task: t.id,
        };
        f.transition(TaskState::AwaitingConfirmation);
        f.transition(TaskState::Confirmed);
        f
    }
    fn transition(&self, state: TaskState) {
        self.db
            .transition_task(&self.task, state, Actor::User, None, None, false, None)
            .unwrap();
    }
    fn awaiting(&self) {
        for state in [
            TaskState::Queued,
            TaskState::Running,
            TaskState::Verifying,
            TaskState::PreviewReady,
            TaskState::AwaitingUserAcceptance,
        ] {
            self.transition(state);
        }
    }
    fn dev(&self, round: i64) {
        let key = format!("dev-{}-round-{round}", self.task);
        self.db
            .plan_action(&self.task, "ai_development", &key)
            .unwrap();
        let a = self.db.start_action(&self.task, &key, false).unwrap();
        self.db
            .finish_action(
                &self.task,
                &key,
                ActionStatus::Succeeded,
                None,
                None,
                a.attempt_number,
            )
            .unwrap();
    }
    fn evidence(&self, generation: i64, round: i64, attempt: i64) {
        self.db
            .record_test_evidence(&RecordTestEvidenceRequest {
                project_id: &self.project,
                task_id: &self.task,
                result_generation: generation,
                round,
                result_attempt: attempt,
                test_command_redacted: None,
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
        self.db
            .record_build_evidence(&RecordBuildEvidenceRequest {
                project_id: &self.project,
                task_id: &self.task,
                result_generation: generation,
                round,
                result_attempt: attempt,
                build_command_redacted: None,
                status: "passed",
                authorization_source: None,
                exit_code: Some(0),
                reason: None,
                stdout_ref: None,
                stderr_ref: None,
            })
            .unwrap();
    }
    fn check(&self, round: i64) {
        let coverage: Vec<_> = self.db.list_confirmed_requirement_items(&self.task).unwrap().iter().map(|i| serde_json::json!({"item": format!("{}:{}",i.field,i.item_index),"status":"covered"})).collect();
        self.db
            .record_self_check(&RecordSelfCheckRequest {
                project_id: &self.project,
                task_id: &self.task,
                round,
                result_attempt: 1,
                parse_status: "valid",
                self_reported_status: Some("self_reported_complete"),
                requirement_coverage_json: Some(&serde_json::to_string(&coverage).unwrap()),
                check_results_json: Some("[]"),
                unfinished_items_json: Some("[]"),
                blocking_issues_json: Some("[]"),
                raw_block_redacted: "regression fixture",
            })
            .unwrap();
    }
    fn events(&self) -> usize {
        self.db
            .list_task_state_events(&self.project, &self.task)
            .unwrap()
            .len()
    }
}

fn authorized_round(f: &Fixture, round: i64) {
    f.db.record_no_tests_confirmation(&f.project, &f.task, 0, round, 1, None)
        .unwrap();
    f.db.record_no_build_confirmation(&f.project, &f.task, 0, round, 1, None)
        .unwrap();
}
fn complete_check(f: &Fixture) -> serde_json::Value {
    let coverage: Vec<_> = f.db.list_confirmed_requirement_items(&f.task).unwrap().iter()
        .map(|i| serde_json::json!({"item":format!("{}:{}",i.field,i.item_index),"status":"covered"})).collect();
    serde_json::json!({"requirement_coverage":coverage,"check_results":[],"unfinished_items":[],"blocking_issues":[],"self_reported_status":"self_reported_complete"})
}
fn wrap_check(value: &serde_json::Value) -> String {
    format!("===NOVA_SELF_CHECK_START===\n{value}\n===NOVA_SELF_CHECK_END===")
}

#[test]
fn wp2a_b1_actual_package_drives_development_and_repair_through_user_acceptance() {
    let f = Fixture::new(false);
    authorized_round(&f, 1);
    authorized_round(&f, 2);
    let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);
    f.transition(TaskState::Queued);
    f.transition(TaskState::Running);
    assert!(matches!(
        run_dev_and_test_round(
            &f.db,
            &f.task,
            &f.project,
            &f.dir,
            ToolKind::CodexCli,
            &adapter,
            &CancellationToken::new(),
            1,
            None,
            &f.dir
        )
        .unwrap(),
        DevTestOutcome::Verified(_)
    ));
    f.transition(TaskState::SafeFixing);
    assert!(matches!(
        run_dev_and_test_round(
            &f.db,
            &f.task,
            &f.project,
            &f.dir,
            ToolKind::CodexCli,
            &adapter,
            &CancellationToken::new(),
            2,
            Some("repair export"),
            &f.dir
        )
        .unwrap(),
        DevTestOutcome::Verified(_)
    ));
    finish_success_path(&f.db, &f.project, &f.task, 2).unwrap();
    let precheck = completion_precheck(&f.db, &f.project, &f.task).unwrap();
    assert!(precheck.can_submit_acceptance);
    assert!(!precheck.ok);
    assert_eq!(
        accept_task_result(&f.db, &f.project, &f.task, true, None, &f.dir)
            .unwrap()
            .state,
        TaskState::Completed
    );
    assert_eq!(adapter.development_call_count(), 2);
    assert_eq!(adapter.review_call_count(), 0);
    for round in [1, 2] {
        let check = f.db.current_self_check(&f.task, round, 1).unwrap().unwrap();
        assert_eq!(check.parse_status, "valid");
        assert!(check.requirement_coverage_json.unwrap().contains("goal:1"));
    }
}

#[test]
fn wp2a_b1_mock_without_dispatched_contract_does_not_fabricate_self_check() {
    let f = Fixture::new(false);
    let path = f.dir.join("legacy-package.md");
    fs::write(&path, "implement export").unwrap();
    let adapter = MockAdapter::new(vec![], vec![]);
    let result = adapter.run_development(
        &DevelopmentRequest {
            ai_kind: ToolKind::CodexCli,
            project_dir: f.dir.clone(),
            package_path: path,
            timeout: DEV_TIMEOUT,
        },
        CancellationToken::new(),
    );
    assert!(matches!(
        self_check_parser::parse_self_check(&result.stdout),
        self_check_parser::ParsedSelfCheck::Invalid { .. }
    ));
}

#[test]
fn wp2a_self_check_rejection_matrix_uses_real_stdout_and_accept_entry() {
    for case in [
        "missing",
        "empty",
        "malformed",
        "unknown_field",
        "unknown_coverage_field",
        "missing_checks",
        "wrong_array",
        "multiple",
        "duplicate_blocks",
        "incomplete",
        "blocked",
        "unfinished",
        "blocking",
        "missing_item",
        "duplicate",
        "fabricated",
        "partial",
        "wrong_key",
    ] {
        let f = Fixture::new(false);
        authorized_round(&f, 1);
        let mut value = complete_check(&f);
        match case {
            "unknown_field" => {
                value["unexpected"] = true.into();
            }
            "unknown_coverage_field" => {
                value["requirement_coverage"][0]["typo"] = true.into();
            }
            "missing_checks" => {
                value.as_object_mut().unwrap().remove("check_results");
            }
            "wrong_array" => {
                value["blocking_issues"] = false.into();
            }
            "incomplete" => value["self_reported_status"] = "self_reported_incomplete".into(),
            "blocked" => value["self_reported_status"] = "self_reported_blocked".into(),
            "unfinished" => value["unfinished_items"] = serde_json::json!(["export unfinished"]),
            "blocking" => value["blocking_issues"] = serde_json::json!(["blocked"]),
            "missing_item" => {
                value["requirement_coverage"].as_array_mut().unwrap().pop();
            }
            "duplicate" => {
                let e = value["requirement_coverage"][0].clone();
                value["requirement_coverage"]
                    .as_array_mut()
                    .unwrap()
                    .push(e);
            }
            "fabricated" => value["requirement_coverage"][0]["item"] = "goal:999".into(),
            "partial" => value["requirement_coverage"][0]["status"] = "partial".into(),
            "wrong_key" => value["requirement_coverage"][0]["item"] = "another_task:0".into(),
            _ => (),
        }
        let stdout = match case {
            "missing" => "work complete".into(),
            "empty" => "===NOVA_SELF_CHECK_START===\n===NOVA_SELF_CHECK_END===".into(),
            "malformed" => "===NOVA_SELF_CHECK_START===\n{broken\n===NOVA_SELF_CHECK_END===".into(),
            "multiple" => {
                let mut contradictory = value.clone();
                contradictory["self_reported_status"] = "self_reported_blocked".into();
                format!("{}\n{}", wrap_check(&value), wrap_check(&contradictory))
            }
            "duplicate_blocks" => format!("{}\n{}", wrap_check(&value), wrap_check(&value)),
            _ => wrap_check(&value),
        };
        let adapter = MockAdapter::new(vec![], vec![]);
        *adapter.development_self_check_override.lock().unwrap() = Some(stdout);
        f.transition(TaskState::Queued);
        f.transition(TaskState::Running);
        let outcome = run_dev_and_test_round(&f.db, &f.task, &f.project, &f.dir,
            ToolKind::CodexCli, &adapter, &CancellationToken::new(), 1, None, &f.dir).unwrap();
        if case == "blocked" {
            assert!(matches!(outcome, DevTestOutcome::Stopped));
            assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::NeedsUserAction);
            assert!(accept_task_result(&f.db, &f.project, &f.task, true, None, &f.dir).is_err());
            continue;
        }
        assert!(matches!(outcome, DevTestOutcome::Verified(_)), "{case}");
        finish_success_path(&f.db, &f.project, &f.task, 1).unwrap();
        assert!(
            !completion_precheck(&f.db, &f.project, &f.task)
                .unwrap()
                .can_submit_acceptance,
            "{case}"
        );
        let events = f.events();
        assert!(
            accept_task_result(&f.db, &f.project, &f.task, true, None, &f.dir).is_err(),
            "{case}"
        );
        assert_eq!(
            f.db.task(&f.task).unwrap().state,
            TaskState::AwaitingUserAcceptance,
            "{case}"
        );
        assert_eq!(events, f.events(), "{case}");
        assert!(!f.db.list_self_checks_for_task(&f.task).unwrap().is_empty());
        // Viewing result documents remains possible after rejected completion.
        documents::generate_result_documents(&f.db, &f.project, &f.task, &f.dir).unwrap();
    }
}

#[test]
fn wp2a_m1_snapshot_state_ownership_idempotence_and_freeze_matrix() {
    for state in [
        TaskState::Draft,
        TaskState::AwaitingConfirmation,
        TaskState::Confirmed,
        TaskState::Queued,
        TaskState::Running,
        TaskState::Verifying,
        TaskState::SafeFixing,
        TaskState::PreviewReady,
        TaskState::AwaitingUserAcceptance,
        TaskState::NeedsUserAction,
        TaskState::Interrupted,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
    ] {
        for existing in [false, true] {
            let f = Fixture::new(false);
            if !existing {
                f.db.conn()
                    .execute(
                        "DELETE FROM confirmed_requirement_items WHERE task_id=?1",
                        [&f.task],
                    )
                    .unwrap();
            }
            f.db.conn().execute("UPDATE tasks SET state=?1,legacy_classification='unknown_insufficient' WHERE id=?2",rusqlite::params![state.as_str(),f.task]).unwrap();
            let before = f.db.list_confirmed_requirement_items(&f.task).unwrap();
            let events = f.events();
            assert!(f
                .db
                .generate_requirement_snapshot("wrong-project", &f.task)
                .is_err());
            let result = f.db.generate_requirement_snapshot(&f.project, &f.task);
            if matches!(
                state,
                TaskState::Draft | TaskState::Completed | TaskState::Failed | TaskState::Cancelled
            ) {
                assert!(result.is_err(), "{state}");
                assert_eq!(
                    before,
                    f.db.list_confirmed_requirement_items(&f.task).unwrap()
                );
            } else {
                let first = result.unwrap();
                assert_eq!(first.len(), 5);
                assert_eq!(
                    first,
                    f.db.generate_requirement_snapshot(&f.project, &f.task)
                        .unwrap()
                );
            }
            assert_eq!(events, f.events());
            assert_eq!(
                f.db.task(&f.task).unwrap().legacy_classification.as_deref(),
                Some("unknown_insufficient")
            );
        }
    }
}

#[test]
fn wp2a_m2_late_attempt_all_successor_states_and_real_sql_error() {
    for kind in ["ai_development", "test_execution", "build_execution"] {
        for state in [
            ActionStatus::Planned,
            ActionStatus::Started,
            ActionStatus::Succeeded,
            ActionStatus::Failed,
            ActionStatus::Cancelled,
            ActionStatus::Uncertain,
        ] {
            let f = Fixture::new(false);
            f.db.plan_action(&f.task, kind, "late").unwrap();
            f.db.start_action(&f.task, "late", false).unwrap();
            f.db.mark_started_actions_uncertain().unwrap();
            f.db.acknowledge_uncertain_action(&f.task, "late").unwrap();
            if state != ActionStatus::Planned {
                f.db.start_action(&f.task, "late", false).unwrap();
            }
            if state == ActionStatus::Uncertain {
                f.db.mark_started_actions_uncertain().unwrap();
            }
            if matches!(
                state,
                ActionStatus::Succeeded | ActionStatus::Failed | ActionStatus::Cancelled
            ) {
                f.db.finish_action(&f.task, "late", state, None, None, 2)
                    .unwrap();
            }
            let before = f.db.find_action(&f.task, "late").unwrap().unwrap();
            assert!(
                matches!(
                    f.db.finish_action(&f.task, "late", ActionStatus::Succeeded, None, None, 1),
                    Err(NovaError::StaleActionAttempt(_))
                ),
                "{kind} {state:?}"
            );
            let after = f.db.find_action(&f.task, "late").unwrap().unwrap();
            assert_eq!(before.status, after.status);
            assert_eq!(before.attempt_number, after.attempt_number);
        }
    }
    let f = Fixture::new(false);
    f.db.plan_action(&f.task, "test_execution", "sql-error")
        .unwrap();
    f.db.start_action(&f.task, "sql-error", false).unwrap();
    f.db.conn().execute_batch("CREATE TRIGGER reject_finish BEFORE UPDATE ON task_actions BEGIN SELECT RAISE(ABORT,'injected finish failure'); END;").unwrap();
    assert!(matches!(
        f.db.finish_action(&f.task, "sql-error", ActionStatus::Succeeded, None, None, 1),
        Err(NovaError::Database(_))
    ));
}

#[test]
fn wp2a_m3_null_and_other_attempt_never_authorize_current_build() {
    let f = Fixture::new(false);
    f.dev(1);
    f.db.record_no_build_confirmation(&f.project, &f.task, 0, 1, 2, None)
        .unwrap();
    for source in [
        Some("legacy_unknown"),
        Some("mechanical_not_applicable"),
        Some("user_confirmed_not_applicable"),
        None,
    ] {
        assert!(!documents::no_build_applicable_is_authorized(
            &f.db, &f.project, &f.task, 0, 1, None, source
        )
        .unwrap());
    }
    assert!(!documents::no_build_applicable_is_authorized(
        &f.db,
        &f.project,
        &f.task,
        0,
        1,
        Some(1),
        Some("user_confirmed_not_applicable")
    )
    .unwrap());
    f.db.record_no_build_confirmation(&f.project, &f.task, 0, 1, 1, None)
        .unwrap();
    assert!(documents::no_build_applicable_is_authorized(
        &f.db,
        &f.project,
        &f.task,
        0,
        1,
        Some(1),
        Some("user_confirmed_not_applicable")
    )
    .unwrap());
    assert_eq!(
        f.db.list_no_build_confirmations_for_round(&f.project, &f.task, 0, 1)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn wp2a_m4_case_c_query_and_explicit_resume_do_not_repeat_confirmation_or_reset() {
    let f = Fixture::new(false);
    f.dev(1);
    f.transition(TaskState::Queued);
    f.transition(TaskState::Running);
    f.transition(TaskState::Verifying);
    f.transition(TaskState::NeedsUserAction);
    let key = build_action_key(&f.task, 0, 1);
    f.db.plan_action(&f.task, "build_execution", &key).unwrap();
    f.db.start_action(&f.task, &key, false).unwrap();
    f.db.finish_action(
        &f.task,
        &key,
        ActionStatus::Failed,
        None,
        Some(BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER),
        1,
    )
    .unwrap();
    f.db.record_build_evidence(&RecordBuildEvidenceRequest {
        project_id: &f.project,
        task_id: &f.task,
        result_generation: 0,
        round: 1,
        result_attempt: 1,
        build_command_redacted: None,
        status: "insufficient_evidence",
        authorization_source: None,
        exit_code: None,
        reason: None,
        stdout_ref: None,
        stderr_ref: None,
    })
    .unwrap();
    let confirmation =
        f.db.record_no_build_confirmation(&f.project, &f.task, 0, 1, 1, None)
            .unwrap();
    f.db.reset_failed_action_for_retry(&f.task, &key).unwrap();
    assert!(no_build_confirmation_needed(&f.db, &f.project, &f.task).unwrap());
    let attempt =
        f.db.find_action(&f.task, &key)
            .unwrap()
            .unwrap()
            .attempt_number;
    let recovered = confirm_no_build_applicable(&f.db, &f.project, &f.task, None).unwrap();
    assert_eq!(confirmation.id, recovered.id);
    assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::Queued);
    assert_eq!(
        attempt,
        f.db.find_action(&f.task, &key)
            .unwrap()
            .unwrap()
            .attempt_number
    );
    assert_eq!(
        f.db.list_no_build_confirmations_for_round(&f.project, &f.task, 0, 1)
            .unwrap()
            .len(),
        1
    );
    assert!(!no_build_confirmation_needed(&f.db, &f.project, &f.task).unwrap());
}

fn prepare_evidence_branch(f: &Fixture, branch: &str) {
    f.dev(1);
    f.transition(TaskState::Queued);
    f.transition(TaskState::Running);
    f.transition(TaskState::Verifying);
    match branch {
        "test_no_tests" => {
            f.db.record_no_tests_confirmation(&f.project, &f.task, 0, 1, 1, None)
                .unwrap();
        }
        "build_confirmed" => {
            f.db.record_no_build_confirmation(&f.project, &f.task, 0, 1, 1, None)
                .unwrap();
        }
        "build_mechanical" => fs::write(f.dir.join("package.json"), "{}").unwrap(),
        "test_allowlist" => {
            f.db.set_task_test_command(&f.task, "forbidden-tool", "user")
                .unwrap();
        }
        "build_allowlist" => test_runner::BUILD_DETECTION_OVERRIDE.with(|v| {
            *v.borrow_mut() = Some(test_runner::BuildDetection::Detected(
                test_runner::DetectedCommand {
                    program: "forbidden-tool".into(),
                    args: vec![],
                },
            ))
        }),
        "test_no_confirmation" | "build_insufficient" => (),
        _ => {
            fs::write(
                f.dir.join("package.json"),
                r#"{"scripts":{"test":"unused","build":"unused"}}"#,
            )
            .unwrap();
            let cancelled = branch.ends_with("cancelled");
            let timed_out = branch.ends_with("timed_out");
            let success = branch.ends_with("passed");
            test_runner::RUN_OUTCOME_OVERRIDE.with(|v| {
                *v.borrow_mut() = Some(test_runner::TestRunOutcome {
                    program: "fake-controlled-runner".into(),
                    args: vec![],
                    cwd: f.dir.to_string_lossy().into_owned(),
                    exit_code: if success { Some(0) } else { Some(1) },
                    passed: Some(if success { 1 } else { 0 }),
                    failed: Some(if success { 0 } else { 1 }),
                    skipped: Some(0),
                    timed_out,
                    cancelled,
                    process_succeeded: success,
                    stdout: "controlled outcome".into(),
                    stderr: String::new(),
                })
            });
        }
    }
}

#[test]
fn wp2a_m6_all_test_build_evidence_branches_cas_before_write_and_late_results_stop() {
    for branch in [
        "test_no_confirmation",
        "test_no_tests",
        "test_allowlist",
        "test_cancelled",
        "test_timed_out",
        "test_passed",
        "test_failed",
        "build_mechanical",
        "build_confirmed",
        "build_insufficient",
        "build_allowlist",
        "build_cancelled",
        "build_timed_out",
        "build_passed",
        "build_failed",
    ] {
        for superseded in [false, true] {
            let f = Fixture::new(false);
            prepare_evidence_branch(&f, branch);
            let table = if branch.starts_with("test") {
                "test_evidence"
            } else {
                "build_evidence"
            };
            let action_type = if branch.starts_with("test") {
                "test_execution"
            } else {
                "build_execution"
            };
            // Each evidence insert proves the real orchestrator finalized the action first.
            f.db.conn().execute_batch(&format!("CREATE TRIGGER assert_cas_before_evidence BEFORE INSERT ON {table} WHEN EXISTS(SELECT 1 FROM task_actions WHERE task_id=NEW.task_id AND action_type='{action_type}' AND status='started') BEGIN SELECT RAISE(ABORT,'evidence before CAS'); END;")).unwrap();
            if superseded {
                // Supersede at the real finish boundary for every branch, including
                // detection/allowlist/cancellation paths that never start a process.
                f.db.conn().execute_batch("CREATE TRIGGER late_finish BEFORE UPDATE ON task_actions WHEN OLD.status='started' AND NEW.status IN ('succeeded','failed','cancelled') BEGIN UPDATE task_actions SET attempt_number=OLD.attempt_number+1,status='planned' WHERE id=OLD.id; SELECT RAISE(IGNORE); END;").unwrap();
            }
            let events = f.events();
            if branch.starts_with("test") {
                let outcome = run_tests_for_round(
                    &f.db,
                    &f.project,
                    &f.task,
                    &f.dir,
                    1,
                    1,
                    &CancellationToken::new(),
                )
                .unwrap();
                if superseded {
                    assert!(matches!(outcome, TestRoundOutcome::Stale), "{branch}");
                }
            } else {
                let outcome = run_build_for_round(
                    &f.db,
                    &f.project,
                    &f.task,
                    &f.dir,
                    1,
                    1,
                    &CancellationToken::new(),
                )
                .unwrap();
                if superseded {
                    assert!(matches!(outcome, BuildRoundOutcome::Stale), "{branch}");
                }
            }
            let count: i64 =
                f.db.conn()
                    .query_row(
                        &format!("SELECT count(*) FROM {table} WHERE task_id=?1"),
                        [&f.task],
                        |r| r.get(0),
                    )
                    .unwrap();
            if superseded {
                assert_eq!(count, 0, "{branch}");
                assert_eq!(events, f.events());
                assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::Verifying);
            } else {
                assert_eq!(
                    count,
                    if branch == "test_no_confirmation" {
                        0
                    } else {
                        1
                    },
                    "{branch}"
                );
            }
        }
    }
}

#[test]
fn wp2a_m6_post_cas_evidence_write_failure_propagates_and_recovery_is_fail_closed() {
    for build in [false, true] {
        let f = Fixture::new(false);
        prepare_evidence_branch(&f, if build { "build_passed" } else { "test_passed" });
        let table = if build {
            "build_evidence"
        } else {
            "test_evidence"
        };
        f.db.conn().execute_batch(&format!("CREATE TRIGGER reject_evidence BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'injected evidence failure'); END;")).unwrap();
        if build {
            assert!(matches!(
                run_build_for_round(
                    &f.db,
                    &f.project,
                    &f.task,
                    &f.dir,
                    1,
                    1,
                    &CancellationToken::new()
                ),
                Err(NovaError::Database(_))
            ));
        } else {
            assert!(matches!(
                run_tests_for_round(
                    &f.db,
                    &f.project,
                    &f.task,
                    &f.dir,
                    1,
                    1,
                    &CancellationToken::new()
                ),
                Err(NovaError::Database(_))
            ));
        }
        let key = if build {
            build_action_key(&f.task, 0, 1)
        } else {
            test_action_key(&f.task, 0, 1)
        };
        assert_eq!(
            f.db.find_action(&f.task, &key).unwrap().unwrap().status,
            ActionStatus::Succeeded
        );
        f.db.conn()
            .execute_batch("DROP TRIGGER reject_evidence;")
            .unwrap();
        // No runner override is supplied: recovery must not execute a process.
        if build {
            assert!(matches!(
                run_build_for_round(
                    &f.db,
                    &f.project,
                    &f.task,
                    &f.dir,
                    1,
                    1,
                    &CancellationToken::new()
                )
                .unwrap(),
                BuildRoundOutcome::ActionUncertain
            ));
        } else {
            assert!(matches!(
                run_tests_for_round(
                    &f.db,
                    &f.project,
                    &f.task,
                    &f.dir,
                    1,
                    1,
                    &CancellationToken::new()
                )
                .unwrap(),
                TestRoundOutcome::ActionUncertain
            ));
        }
        assert_eq!(
            f.db.task(&f.task).unwrap().state,
            TaskState::NeedsUserAction
        );
        let status: String =
            f.db.conn()
                .query_row(
                    &format!("SELECT status FROM {table} WHERE task_id=?1"),
                    [&f.task],
                    |r| r.get(0),
                )
                .unwrap();
        assert_eq!(status, "insufficient_evidence");
    }
}

#[test]
fn wp2a_m6_frozen_execution_confirmation_retry_and_completion_are_zero_write() {
    for classification in ["unknown_insufficient", "dual_ai_evidenced"] {
        let f = Fixture::new(false);
        f.db.conn()
            .execute(
                "UPDATE tasks SET legacy_classification=?1 WHERE id=?2",
                rusqlite::params![classification, f.task],
            )
            .unwrap();
        let adapter = MockAdapter::new(vec![], vec![]);
        let events = f.events();
        assert!(
            run_core_loop(&f.db, &f.task, &adapter, &CancellationToken::new(), &f.dir).is_err()
        );
        assert!(confirm_no_tests_applicable(&f.db, &f.project, &f.task, None).is_err());
        assert!(confirm_no_build_applicable(&f.db, &f.project, &f.task, None).is_err());
        assert!(unblock_test_execution_for_retry(&f.db, &f.project, &f.task).is_err());
        for state in [TaskState::Queued, TaskState::Running, TaskState::Completed] {
            assert!(f
                .db
                .transition_task(&f.task, state, Actor::User, None, None, false, None)
                .is_err());
        }
        assert_eq!(adapter.development_call_count(), 0);
        assert_eq!(events, f.events());
        for table in [
            "task_actions",
            "no_tests_confirmations",
            "no_build_confirmations",
        ] {
            let count: i64 =
                f.db.conn()
                    .query_row(
                        &format!("SELECT count(*) FROM {table} WHERE task_id=?1"),
                        [&f.task],
                        |r| r.get(0),
                    )
                    .unwrap();
            assert_eq!(count, 0);
        }
        // Snapshot generation remains allowed and cannot thaw the task.
        assert_eq!(
            f.db.generate_requirement_snapshot(&f.project, &f.task)
                .unwrap()
                .len(),
            5
        );
        assert_eq!(
            f.db.task(&f.task).unwrap().legacy_classification.as_deref(),
            Some(classification)
        );
    }
}

#[test]
fn wp2a_m1_snapshot_insert_failure_rolls_back_every_item() {
    let f = Fixture::new(false);
    f.db.conn()
        .execute(
            "DELETE FROM confirmed_requirement_items WHERE task_id=?1",
            [&f.task],
        )
        .unwrap();
    f.db.conn().execute_batch("CREATE TRIGGER reject_second_item BEFORE INSERT ON confirmed_requirement_items WHEN NEW.item_index=1 BEGIN SELECT RAISE(ABORT,'snapshot failure'); END;").unwrap();
    assert!(matches!(
        f.db.generate_requirement_snapshot(&f.project, &f.task),
        Err(NovaError::Database(_))
    ));
    assert!(f
        .db
        .list_confirmed_requirement_items(&f.task)
        .unwrap()
        .is_empty());
    assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::Confirmed);
}

#[test]
fn wp2a_m2_real_development_late_results_stop_for_every_successor_state() {
    for state in [
        ActionStatus::Planned,
        ActionStatus::Started,
        ActionStatus::Succeeded,
        ActionStatus::Failed,
        ActionStatus::Cancelled,
        ActionStatus::Uncertain,
    ] {
        let f = Fixture::new(true);
        f.transition(TaskState::Queued);
        f.transition(TaskState::Running);
        let path = f.dir.join("test.sqlite3");
        let task = f.task.clone();
        let adapter = MockAdapter::new(vec![], vec![]);
        *adapter.development_side_effect.lock().unwrap() = Some(Box::new(move |_| {
            let db = Database::open(&path).unwrap();
            let key = format!("dev-{task}-round-1");
            db.mark_started_actions_uncertain().unwrap();
            db.acknowledge_uncertain_action(&task, &key).unwrap();
            if state != ActionStatus::Planned {
                db.start_action(&task, &key, false).unwrap();
            }
            if state == ActionStatus::Uncertain {
                db.mark_started_actions_uncertain().unwrap();
            }
            if matches!(
                state,
                ActionStatus::Succeeded | ActionStatus::Failed | ActionStatus::Cancelled
            ) {
                db.finish_action(&task, &key, state, None, None, 2).unwrap();
            }
        }));
        let events = f.events();
        assert!(
            matches!(
                run_dev_and_test_round(
                    &f.db,
                    &f.task,
                    &f.project,
                    &f.dir,
                    ToolKind::CodexCli,
                    &adapter,
                    &CancellationToken::new(),
                    1,
                    None,
                    &f.dir
                )
                .unwrap(),
                DevTestOutcome::Stale
            ),
            "{state:?}"
        );
        assert_eq!(events, f.events());
        assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::Running);
        for table in ["self_checks", "test_evidence", "build_evidence"] {
            let count: i64 =
                f.db.conn()
                    .query_row(
                        &format!("SELECT count(*) FROM {table} WHERE task_id=?1"),
                        [&f.task],
                        |r| r.get(0),
                    )
                    .unwrap();
            assert_eq!(count, 0);
        }
    }
}

#[test]
fn wp2a_m1_empty_snapshot_stays_empty_and_cannot_complete() {
    let f = Fixture::new(false);
    f.db.conn()
        .execute(
            "DELETE FROM confirmed_requirement_items WHERE task_id=?1",
            [&f.task],
        )
        .unwrap();
    f.db.conn().execute("UPDATE tasks SET goal=NULL,in_scope='  ',not_in_scope=NULL,done_criteria=NULL WHERE id=?1",[&f.task]).unwrap();
    assert!(f
        .db
        .generate_requirement_snapshot(&f.project, &f.task)
        .unwrap()
        .is_empty());
    assert!(write_task_package(&f.db, &f.task, &f.project, 1, None, &f.dir).is_err());
    f.awaiting();
    f.dev(1);
    f.evidence(0, 1, 1);
    f.check(1);
    assert!(
        !completion_precheck(&f.db, &f.project, &f.task)
            .unwrap()
            .can_submit_acceptance
    );
}

#[test]
fn wp2a_m1_tauri_snapshot_command_is_registered_with_ownership_parameters() {
    let commands = include_str!("../commands.rs");
    let lib = include_str!("../lib.rs");
    assert!(commands.contains("pub fn generate_requirement_snapshot("));
    assert!(commands.contains("db.generate_requirement_snapshot(&project_id, &task_id)"));
    assert!(lib.contains("commands::generate_requirement_snapshot,"));
}

#[test]
fn wp2a_m1_m3_timeline_uses_dev_attribution_and_marks_same_generation_old_attempt() {
    let f = Fixture::new(false);
    f.dev(1);
    f.evidence(0, 1, 1);
    for (kind, key) in [
        ("test_execution", test_action_key(&f.task, 0, 1)),
        ("build_execution", build_action_key(&f.task, 0, 1)),
    ] {
        f.db.plan_action(&f.task, kind, &key).unwrap();
        f.db.start_action(&f.task, &key, false).unwrap();
        f.db.finish_action(&f.task, &key, ActionStatus::Failed, None, None, 1)
            .unwrap();
        f.db.reset_failed_action_for_retry(&f.task, &key).unwrap();
    }
    f.db.record_no_tests_confirmation(&f.project, &f.task, 0, 1, 1, None)
        .unwrap();
    f.db.record_no_tests_confirmation(&f.project, &f.task, 0, 1, 99, None)
        .unwrap();
    f.db.record_no_build_confirmation(&f.project, &f.task, 0, 1, 1, None)
        .unwrap();
    let overview = task_evidence_overview(&f.db, &f.project, &f.task).unwrap();
    let confirmations: Vec<_> = overview
        .timeline
        .iter()
        .filter(|e| e.event_type == "no_tests_confirmed" || e.event_type == "no_build_confirmed")
        .collect();
    assert_eq!(confirmations.len(), 3);
    assert_eq!(confirmations.iter().filter(|e| !e.is_historical).count(), 2);
    assert!(overview.tests[0].is_current_attempt);
    let key = format!("dev-{}-round-1", f.task);
    f.db.conn()
        .execute(
            "UPDATE task_actions SET attempt_number=2 WHERE task_id=?1 AND idempotency_key=?2",
            rusqlite::params![f.task, key],
        )
        .unwrap();
    let overview = task_evidence_overview(&f.db, &f.project, &f.task).unwrap();
    assert!(!overview.tests[0].is_current_attempt);
    assert!(overview
        .timeline
        .iter()
        .filter(|e| e.event_type == "test_evidence"
            || e.event_type == "build_evidence"
            || e.event_type == "no_tests_confirmed"
            || e.event_type == "no_build_confirmed")
        .all(|e| e.is_historical));
}

#[test]
fn wp2a_frozen_existing_failed_actions_and_acceptance_are_unchanged_on_rejection() {
    for classification in ["unknown_insufficient", "dual_ai_evidenced"] {
        let f = Fixture::new(false);
        f.dev(1);
        f.transition(TaskState::Queued);
        f.transition(TaskState::Running);
        f.transition(TaskState::Verifying);
        f.transition(TaskState::NeedsUserAction);
        for (kind, key, marker) in [
            (
                "test_execution",
                test_action_key(&f.task, 0, 1),
                NO_TESTS_AWAITING_CONFIRMATION_MARKER,
            ),
            (
                "build_execution",
                build_action_key(&f.task, 0, 1),
                BUILD_INSUFFICIENT_AWAITING_CONFIRMATION_MARKER,
            ),
        ] {
            f.db.plan_action(&f.task, kind, &key).unwrap();
            f.db.start_action(&f.task, &key, false).unwrap();
            f.db.finish_action(&f.task, &key, ActionStatus::Failed, None, Some(marker), 1)
                .unwrap();
        }
        f.db.conn()
            .execute(
                "UPDATE tasks SET legacy_classification=?1 WHERE id=?2",
                rusqlite::params![classification, f.task],
            )
            .unwrap();
        let before = serde_json::to_value(f.db.list_actions_for_task(&f.task).unwrap()).unwrap();
        let events = f.events();
        assert!(confirm_no_tests_applicable(&f.db, &f.project, &f.task, None).is_err());
        assert!(confirm_no_build_applicable(&f.db, &f.project, &f.task, None).is_err());
        assert!(unblock_test_execution_for_retry(&f.db, &f.project, &f.task).is_err());
        assert_eq!(
            before,
            serde_json::to_value(f.db.list_actions_for_task(&f.task).unwrap()).unwrap()
        );
        assert_eq!(events, f.events());
        f.db.conn()
            .execute(
                "UPDATE tasks SET state='awaiting_user_acceptance' WHERE id=?1",
                [&f.task],
            )
            .unwrap();
        let events = f.events();
        assert!(accept_task_result(&f.db, &f.project, &f.task, true, None, &f.dir).is_err());
        assert_eq!(events, f.events());
        let count: i64 =
            f.db.conn()
                .query_row(
                    "SELECT count(*) FROM user_acceptance WHERE task_id=?1",
                    [&f.task],
                    |r| r.get(0),
                )
                .unwrap();
        assert_eq!(count, 0);
    }
}

#[test]
fn wp2a_cached_evidence_for_another_dev_attempt_stops_test_and_build_recovery() {
    for build in [false, true] {
        let f = Fixture::new(false);
        f.dev(1);
        f.transition(TaskState::Queued);
        f.transition(TaskState::Running);
        f.transition(TaskState::Verifying);
        f.evidence(0, 1, 99);
        let key = if build {
            build_action_key(&f.task, 0, 1)
        } else {
            test_action_key(&f.task, 0, 1)
        };
        let kind = if build {
            "build_execution"
        } else {
            "test_execution"
        };
        f.db.plan_action(&f.task, kind, &key).unwrap();
        f.db.start_action(&f.task, &key, false).unwrap();
        f.db.finish_action(&f.task, &key, ActionStatus::Succeeded, None, None, 1)
            .unwrap();
        if build {
            assert!(matches!(
                run_build_for_round(
                    &f.db,
                    &f.project,
                    &f.task,
                    &f.dir,
                    1,
                    1,
                    &CancellationToken::new()
                )
                .unwrap(),
                BuildRoundOutcome::ActionUncertain
            ));
        } else {
            assert!(matches!(
                run_tests_for_round(
                    &f.db,
                    &f.project,
                    &f.task,
                    &f.dir,
                    1,
                    1,
                    &CancellationToken::new()
                )
                .unwrap(),
                TestRoundOutcome::ActionUncertain
            ));
        }
        assert_eq!(
            f.db.task(&f.task).unwrap().state,
            TaskState::NeedsUserAction
        );
    }
}

#[test]
fn wp2a_b1_development_and_fix_packages_include_snapshot_and_stdout_contract() {
    let f = Fixture::new(false);
    for round in [1, 2] {
        let path =
            write_task_package(&f.db, &f.task, &f.project, round, Some("fix CSV"), &f.dir).unwrap();
        let body = fs::read_to_string(path).unwrap();
        for required in [
            "===NOVA_REQUIREMENT_ITEMS_START===",
            "goal:0",
            "goal:1",
            "in_scope:0",
            "not_in_scope:0",
            "done_criteria:0",
            "===NOVA_SELF_CHECK_START===",
            "===NOVA_SELF_CHECK_END===",
            "requirement_coverage",
            "check_results",
            "unfinished_items",
            "blocking_issues",
            "self_reported_complete",
            "self_reported_incomplete",
            "self_reported_blocked",
            "stdout",
        ] {
            assert!(body.contains(required), "round {round} missing {required}");
        }
    }
}

#[test]
fn wp2a_b2_fixed_adoption_cannot_fall_back_to_older_generation_or_round() {
    let f = Fixture::new(false);
    f.awaiting();
    f.dev(1);
    f.dev(2);
    f.evidence(1, 1, 99);
    f.evidence(0, 2, 1);
    f.check(2);
    f.db.record_user_acceptance(&f.project, &f.task, true, None)
        .unwrap();
    let events = f.events();
    assert!(f
        .db
        .transition_task(
            &f.task,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None
        )
        .is_err());
    assert_eq!(
        f.db.task(&f.task).unwrap().state,
        TaskState::AwaitingUserAcceptance
    );
    assert_eq!(events, f.events());
}

#[test]
fn wp2a_b3_two_connection_generation_aba_rechecks_current_acceptance() {
    let f = Fixture::new(true);
    f.awaiting();
    f.dev(1);
    f.evidence(1, 1, 1);
    f.check(1);
    f.db.record_user_acceptance(&f.project, &f.task, true, None)
        .unwrap();
    let arrived = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let a = arrived.clone();
    let r = resume.clone();
    let path = f.dir.join("test.sqlite3");
    let task = f.task.clone();
    let worker = std::thread::spawn(move || {
        let db = Database::open(&path).unwrap();
        crate::storage::tasks::BEFORE_TRANSITION_LOCK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                a.wait();
                r.wait();
            }))
        });
        db.transition_task(
            &task,
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
    });
    arrived.wait();
    // Actual business transitions, including the atomic PreviewReady generation bump.
    for state in [
        TaskState::SafeFixing,
        TaskState::Verifying,
        TaskState::PreviewReady,
        TaskState::AwaitingUserAcceptance,
    ] {
        f.transition(state);
    }
    assert_eq!(f.db.task(&f.task).unwrap().result_generation, 2);
    let events = f.events();
    resume.wait();
    assert!(
        worker.join().unwrap().is_err(),
        "generation 1 acceptance must not complete generation 2"
    );
    assert_eq!(
        f.db.task(&f.task).unwrap().state,
        TaskState::AwaitingUserAcceptance
    );
    assert!(f.db.user_acceptance_for_task(&f.task).unwrap().is_none());
    assert_eq!(events, f.events());
}


#[test]
fn rav01_exit_zero_but_blocked_stops_before_tests_and_resume() {
    let f = Fixture::new(false);
    let mut check = complete_check(&f);
    check["self_reported_status"] = "self_reported_blocked".into();
    check["blocking_issues"] = serde_json::json!(["helper_unknown_error: setup refresh had errors"]);
    let adapter = MockAdapter::new(vec![MockOutcome::Success], vec![]);
    *adapter.development_self_check_override.lock().unwrap() = Some(wrap_check(&check));
    f.transition(TaskState::Queued); f.transition(TaskState::Running);
    for n in 0..2 {
        if n == 1 { f.transition(TaskState::Queued); f.transition(TaskState::Running); }
        assert!(matches!(run_dev_and_test_round(&f.db, &f.task, &f.project,
            &f.dir, ToolKind::CodexCli, &adapter, &CancellationToken::new(), 1, None, &f.dir).unwrap(),
            DevTestOutcome::Stopped));
        assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::NeedsUserAction);
        assert_eq!(f.db.list_actions_for_task(&f.task).unwrap().len(), 1);
        assert!(f.db.list_test_evidence_for_round(&f.project, &f.task, 0, 1).unwrap().is_empty());
    }
    assert_eq!(adapter.development_call_count(), 1); assert_eq!(adapter.review_call_count(), 0);
    assert!(f.db.list_task_state_events(&f.project, &f.task).unwrap().iter()
        .any(|e| e.reason.as_deref().unwrap_or("").contains("helper_unknown_error")));
}

#[test]
fn rav01_test_cleanup_cannot_overwrite_a_new_attempt_or_terminal_row() {
    let f = Fixture::new(false); let key = test_action_key(&f.task, 0, 1);
    f.db.plan_action(&f.task, "test_execution", &key).unwrap();
    let first = f.db.start_action(&f.task, &key, false).unwrap();
    drop(TestActionGuard { db: &f.db, task_id: &f.task, key: &key, attempt: first.attempt_number });
    let stopped = f.db.find_action(&f.task, &key).unwrap().unwrap();
    assert_eq!(stopped.status, ActionStatus::Failed); assert!(stopped.finished_at.is_some());
    assert!(stopped.error_summary.is_some());
    f.db.reset_failed_action_for_retry(&f.task, &key).unwrap();
    let second = f.db.start_action(&f.task, &key, false).unwrap();
    drop(TestActionGuard { db: &f.db, task_id: &f.task, key: &key, attempt: first.attempt_number });
    assert_eq!(f.db.find_action(&f.task, &key).unwrap().unwrap().status, ActionStatus::Started);
    f.db.finish_action(&f.task, &key, ActionStatus::Succeeded, None, None, second.attempt_number).unwrap();
    drop(TestActionGuard { db: &f.db, task_id: &f.task, key: &key, attempt: second.attempt_number });
    assert_eq!(f.db.find_action(&f.task, &key).unwrap().unwrap().status, ActionStatus::Succeeded);
}

#[test]
fn rav01_unexpected_run_error_and_panic_are_visible() {
    for panic in [false, true] {
        let f = Fixture::new(false); f.transition(TaskState::Queued); f.transition(TaskState::Running);
        let result = report_run_failure(&f.db, &f.task, || {
            if panic { panic!("injected failure") }
            Err(NovaError::Config("injected worker error".into()))
        });
        assert!(result.is_err()); assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::NeedsUserAction);
        let events = f.db.list_task_state_events(&f.project, &f.task).unwrap();
        assert!(events.last().unwrap().reason.as_deref().unwrap().contains("执行受阻"));
    }
}

#[test]
fn rav01_failed_test_settles_ledger_and_keeps_spawn_reason() {
    let f = Fixture::new(false); fs::write(f.dir.join("package.json"), "{}").unwrap();
    f.transition(TaskState::Queued); f.transition(TaskState::Running); f.transition(TaskState::Verifying);
    test_runner::RUN_OUTCOME_OVERRIDE.with(|slot| *slot.borrow_mut() = Some(test_runner::TestRunOutcome {
        program: "npm".into(), args: vec!["test".into()], cwd: f.dir.to_string_lossy().into_owned(),
        exit_code: None, passed: None, failed: None, skipped: None, timed_out: false,
        cancelled: false, process_succeeded: false, stdout: String::new(), stderr: "cannot start process".into(),
    }));
    assert!(matches!(run_tests_for_round(&f.db, &f.project, &f.task, &f.dir, 1, 1,
        &CancellationToken::new()).unwrap(), TestRoundOutcome::ActionUncertain));
    assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::NeedsUserAction);
    let action = f.db.find_action(&f.task, &test_action_key(&f.task, 0, 1)).unwrap().unwrap();
    assert_eq!(action.status, ActionStatus::Failed); assert!(action.finished_at.is_some());
    assert!(action.error_summary.unwrap().contains("cannot start process"));
    let evidence = f.db.list_test_evidence_for_round(&f.project, &f.task, 0, 1).unwrap();
    assert_eq!(evidence[0].status, "failed");
    assert!(evidence[0].reason.as_deref().unwrap().contains("cannot start process"));
}


#[test]
fn rav01_test_start_failure_never_invokes_automatic_ai_repair() {
    let f = Fixture::new(false);
    fs::write(f.dir.join("package.json"), r#"{"scripts":{"test":"node absent.mjs","build":"node absent.mjs"}}"#).unwrap();
    let adapter = MockAdapter::new(vec![MockOutcome::Success, MockOutcome::Success], vec![]);
    test_runner::RUN_OUTCOME_OVERRIDE.with(|slot| *slot.borrow_mut() = Some(test_runner::TestRunOutcome {
        program: "npm".into(), args: vec!["test".into()], cwd: f.dir.to_string_lossy().into_owned(),
        exit_code: None, passed: None, failed: None, skipped: None, timed_out: false,
        cancelled: false, process_succeeded: false, stdout: String::new(), stderr: "process unavailable".into(),
    }));
    let outcome = run_core_loop(&f.db, &f.task, &adapter, &CancellationToken::new(), &f.dir).unwrap();
    assert_eq!(outcome.task.state, TaskState::NeedsUserAction);
    assert_eq!(adapter.development_call_count(), 1);
    assert_eq!(adapter.review_call_count(), 0);
    assert!(f.db.list_actions_for_task(&f.task).unwrap().iter().all(|a| a.status != ActionStatus::Started));
}
