use super::*;
use crate::{
    credential::MemoryCredentialStore,
    models::{Actor, CreateProjectRequest, CreateTaskRequest, TaskState},
};
struct Fixture {
    db: Database,
    vault: MemoryCredentialStore,
    root: PathBuf,
    task: String,
    config: Config,
}
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/api-only-tests")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("note.txt"), b"Alpha \xc3\x84\r\nend\r\n").unwrap();
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let project = db
            .create_project(&CreateProjectRequest {
                name: "API fixture".into(),
                local_path: root.to_string_lossy().into(),
                source_type: "local".into(),
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
                    project_id: project.id,
                    title: "Change one word".into(),
                    goal: Some("Change Alpha to beta".into()),
                    in_scope: Some("note.txt only".into()),
                    not_in_scope: Some("No other writes".into()),
                    done_criteria: Some("Contains beta".into()),
                    ai_kind: Some("claude_cli".into()),
                },
                Actor::User,
            )
            .unwrap();
        for state in [TaskState::AwaitingConfirmation, TaskState::Confirmed] {
            db.transition_task(&task.id, state, Actor::User, None, None, false, None)
                .unwrap();
        }
        let vault = MemoryCredentialStore::default();
        let config = Config {
            id: uuid::Uuid::new_v4().to_string(),
            provider: Provider::Deepseek,
            base_url: "https://api.deepseek.com/v1".into(),
            model: "fixture-model-selected-by-user".into(),
        };
        config::save_config(
            &db,
            &vault,
            config.clone(),
            Some("SYNTHETIC_SECRET_VALUE_12345678".into()),
        )
        .unwrap();
        Self {
            db,
            vault,
            root,
            task: task.id,
            config,
        }
    }
    fn test(&self) -> FixedTest {
        FixedTest {
            program: std::env::var_os("NOVA_API_TEST_PYTHON")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("PATH").and_then(|v| {
                        std::env::split_paths(&v)
                            .map(|p| p.join("python.exe"))
                            .find(|p| p.is_file())
                    })
                })
                .expect("synthetic content-verifier test requires an installed Python executable"),
            file: "note.txt".into(),
            contains: "beta".into(),
        }
    }
    fn plan(&self) -> Plan {
        prepare(
            &self.db,
            &self.vault,
            &self.task,
            &self.config.id,
            vec!["note.txt".into()],
            vec![self.test()],
        )
        .unwrap()
    }
    fn approved(&self) -> Plan {
        let p = self.plan();
        approve(&self.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
        p
    }
}
#[derive(Default)]
struct FakeApi {
    mode: &'static str,
    calls: Vec<Value>,
    root: PathBuf,
}
impl ApiPort for FakeApi {
    fn send(
        &mut self,
        r: &transport::WireRequest,
        _: &SecretLease,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>> {
        if r.method == "GET" {
            self.calls.push(json!({"connect":true}));
            return Ok(br#"{"data":[{"id":"fixture-model-selected-by-user"}]}"#.to_vec());
        }
        let body: Value = decode(&r.body, 131072)?;
        self.calls.push(body.clone());
        let input: Value = decode(
            body["messages"][1]["content"].as_str().unwrap().as_bytes(),
            131072,
        )?;
        if self.mode == "timeout" {
            return Err("API_TIMEOUT_UNKNOWN");
        }
        if self.mode == "cancel" {
            cancel.cancel();
            return Err("API_CANCELLED_UNKNOWN");
        }
        if self.mode == "invalid" {
            return Ok(b"not-json".to_vec());
        }
        let content = if self.calls.len() == 1 {
            if self.mode == "multi" {
                let edits:Vec<_>=input["files"].as_array().unwrap().iter().map(|f|json!({"schema":"nova.relay.proposal/1","target_path":f["path"],"old_text":"Alpha","new_text":"beta","rationale":"Approved word change"})).collect();
                return Ok(serde_json::to_vec(&json!({"model":body["model"],"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":encode(&json!({"schema":"nova.api.edits/1","edits":edits}))?}}]})).unwrap());
            }
            if self.mode == "drift" {
                std::fs::write(self.root.join("note.txt"), "USER_CHANGED").unwrap();
            }
            let path = if self.mode == "outside" {
                "../other.txt"
            } else {
                "note.txt"
            };
            let replacement = if self.mode == "secret" {
                "SYNTHETIC_SECRET_VALUE_12345678"
            } else {
                "beta"
            };
            json!({"schema":"nova.api.edits/1","edits":[{"schema":"nova.relay.proposal/1","target_path":path,"old_text":"Alpha","new_text":replacement,"rationale":"Approved word change"}]})
        } else {
            assert_eq!(
                std::fs::read(self.root.join("note.txt")).unwrap(),
                b"beta \xc3\x84\r\nend\r\n"
            );
            assert!(input.get("actual_tests").is_some());
            assert_eq!(body["messages"].as_array().unwrap().len(), 2);
            assert!(body.get("tools").is_none());
            json!({"schema":"nova.relay.review/1","base_sha256":input["base_sha256"],"candidate_sha256":input["candidate_sha256"],"verdict":if self.mode=="changes"{"CHANGES_REQUIRED"}else{"PASS"},"issues":[],"unknowns":[]})
        };
        Ok(serde_json::to_vec(&json!({"model":body["model"],"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":encode(&content)?}}]})).unwrap())
    }
}
struct FakeTests {
    state: &'static str,
    calls: usize,
}
impl TestPort for FakeTests {
    fn run(&mut self, t: &ApprovedTest, _: &Path, _: &CancellationToken) -> Result<TestEvidence> {
        self.calls += 1;
        Ok(TestEvidence {
            file: t.specification.file.clone(),
            status: self.state.into(),
            exit_code: if self.state == "PASS" {
                Some(0)
            } else {
                Some(1)
            },
            stdout: "SYNTHETIC_TEST".into(),
            stderr: String::new(),
        })
    }
}
fn execute_case(
    f: &Fixture,
    p: &Plan,
    mode: &'static str,
    test_state: &'static str,
) -> (Value, FakeApi, FakeTests) {
    let mut api = FakeApi {
        mode,
        root: f.root.clone(),
        ..Default::default()
    };
    let mut tests = FakeTests {
        state: test_state,
        calls: 0,
    };
    let result = execute(
        &f.db,
        &p.id,
        &f.vault,
        &mut api,
        &mut tests,
        &CancellationToken::new(),
    )
    .unwrap();
    (result, api, tests)
}
#[test]
fn product_vertical_flow_real_file_and_independent_review() {
    let f = Fixture::new();
    let p = f.approved();
    let (r, api, tests) = execute_case(&f, &p, "ok", "PASS");
    assert_eq!(r["phase"], "AWAITING_USER");
    assert_eq!(tests.calls, 1);
    assert_eq!(api.calls.len(), 2);
    assert!(api.calls.iter().all(|v| v["model"] == f.config.model));
    assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::Confirmed);
    assert_eq!(
        r["summary"]["changes"][0]["candidate"]["text"],
        "beta Ä\r\nend\r\n"
    );
    assert_eq!(
        std::fs::read(f.root.join("note.txt")).unwrap(),
        b"beta \xc3\x84\r\nend\r\n"
    );
}
#[test]
fn readonly_fixed_test_runs_without_project_code() {
    let f = Fixture::new();
    let p = f.approved();
    let mut api = FakeApi {
        root: f.root.clone(),
        ..Default::default()
    };
    let r = execute(
        &f.db,
        &p.id,
        &f.vault,
        &mut api,
        &mut FixedTestRunner,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(r["phase"], "AWAITING_USER");
    assert_eq!(
        r["summary"]["tests"][0]["stdout"],
        "CONTENT_ASSERT_PASS\r\n"
    );
}
#[test]
fn invalid_response_and_outside_proposal_do_not_write() {
    for mode in ["invalid", "outside"] {
        let f = Fixture::new();
        let p = f.approved();
        let (r, api, t) = execute_case(&f, &p, mode, "PASS");
        assert_eq!(r["phase"], "FAILED");
        assert_eq!(api.calls.len(), 1);
        assert_eq!(t.calls, 0);
        assert_eq!(
            std::fs::read_to_string(f.root.join("note.txt")).unwrap(),
            p.files[0].before
        );
    }
}
#[test]
fn path_scope_rejects_credentials_traversal_and_duplicates() {
    let f = Fixture::new();
    for path in [
        "../note.txt",
        ".env",
        "nested/auth.json",
        "x.pem",
        ".git/config",
    ] {
        assert!(prepare(
            &f.db,
            &f.vault,
            &f.task,
            &f.config.id,
            vec![path.into()],
            vec![f.test()]
        )
        .is_err());
    }
    assert!(prepare(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into(), "NOTE.txt".into()],
        vec![f.test()]
    )
    .is_err());
}
#[test]
fn file_drift_preserves_users_work() {
    let f = Fixture::new();
    let p = f.approved();
    let (r, _, t) = execute_case(&f, &p, "drift", "PASS");
    assert_eq!(r["phase"], "FAILED");
    assert_eq!(t.calls, 0);
    assert_eq!(
        std::fs::read_to_string(f.root.join("note.txt")).unwrap(),
        "USER_CHANGED"
    );
}
#[test]
fn tests_failed_or_review_changes_never_complete() {
    for (mode, test) in [("ok", "FAIL"), ("changes", "PASS")] {
        let f = Fixture::new();
        let p = f.approved();
        let (r, api, _) = execute_case(&f, &p, mode, test);
        assert_eq!(r["phase"], "NEEDS_REMEDIATION");
        assert_eq!(api.calls.len(), 2);
        assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::Confirmed);
    }
}
#[test]
fn cancellation_timeout_and_unknown_are_not_replayed() {
    for mode in ["cancel", "timeout"] {
        let f = Fixture::new();
        let p = f.approved();
        let (r, mut api, mut tests) = execute_case(&f, &p, mode, "PASS");
        assert_eq!(r["phase"], "UNKNOWN");
        assert_eq!(api.calls.len(), 1);
        assert_eq!(
            execute(
                &f.db,
                &p.id,
                &f.vault,
                &mut api,
                &mut tests,
                &CancellationToken::new()
            )
            .unwrap_err(),
            "API_RUN_ALREADY_STARTED"
        );
        assert_eq!(api.calls.len(), 1);
        assert!(f.plan_error_is_serial());
    }
}
impl Fixture {
    fn plan_error_is_serial(&self) -> bool {
        prepare(
            &self.db,
            &self.vault,
            &self.task,
            &self.config.id,
            vec!["note.txt".into()],
            vec![self.test()],
        )
        .is_err()
    }
}
#[test]
fn unknown_tests_stop_before_review() {
    let f = Fixture::new();
    let p = f.approved();
    let (r, api, _) = execute_case(&f, &p, "ok", "UNKNOWN");
    assert_eq!(r["phase"], "UNKNOWN");
    assert_eq!(api.calls.len(), 1);
}
#[test]
fn key_never_enters_request_or_database_evidence() {
    let f = Fixture::new();
    let p = f.approved();
    let (r, api, t) = execute_case(&f, &p, "secret", "PASS");
    assert_eq!(r["phase"], "FAILED");
    assert_eq!(t.calls, 0);
    let saved: String =
        f.db.conn()
            .query_row(
                "SELECT plan_json||summary_json FROM api_task_runs WHERE id=?1",
                [p.id],
                |r| r.get(0),
            )
            .unwrap();
    assert!(!saved.contains("SYNTHETIC_SECRET_VALUE_12345678"));
    assert!(!encode(&api.calls)
        .unwrap()
        .contains("SYNTHETIC_SECRET_VALUE_12345678"));
}
#[test]
fn approval_missing_wrong_digest_and_config_drift_reject() {
    let f = Fixture::new();
    let p = f.plan();
    assert!(approve(&f.db, &p.id, "wrong").is_err());
    let mut api = FakeApi::default();
    let mut tests = FakeTests {
        state: "PASS",
        calls: 0,
    };
    assert!(execute(
        &f.db,
        &p.id,
        &f.vault,
        &mut api,
        &mut tests,
        &CancellationToken::new()
    )
    .is_err());
    let d = hash(encode(&p).unwrap().as_bytes());
    approve(&f.db, &p.id, &d).unwrap();
    config::save_config(&f.db, &f.vault, f.config.clone(), None).unwrap();
    assert_eq!(
        execute(
            &f.db,
            &p.id,
            &f.vault,
            &mut api,
            &mut tests,
            &CancellationToken::new()
        )
        .unwrap_err(),
        "API_BINDING_DRIFT"
    );
    assert!(api.calls.is_empty());
}
#[test]
fn providers_have_distinct_protocols_and_unimplemented_do_not_send() {
    let f = Fixture::new();
    let mut api = FakeApi::default();
    for (provider, base, protocol) in [
        (
            Provider::Sdkmax,
            "https://api.sdkmax.com/v1",
            Protocol::OpenaiChat,
        ),
        (
            Provider::Openai,
            "https://api.openai.com/v1",
            Protocol::OpenaiResponses,
        ),
        (
            Provider::Claude,
            "https://api.anthropic.com/v1",
            Protocol::AnthropicMessages,
        ),
        (
            Provider::Kimi,
            "https://api.moonshot.ai/v1",
            Protocol::OpenaiChat,
        ),
        (
            Provider::Gemini,
            "https://generativelanguage.googleapis.com/v1beta",
            Protocol::GeminiContent,
        ),
        (
            Provider::OpenaiCompatible,
            "https://example.com:8443/custom/v1",
            Protocol::OpenaiChat,
        ),
    ] {
        let c = Config {
            id: uuid::Uuid::new_v4().to_string(),
            provider,
            base_url: base.into(),
            model: "explicit".into(),
        };
        assert_eq!(c.protocol(), protocol);
        config::save_config(
            &f.db,
            &f.vault,
            c.clone(),
            Some("SYNTHETIC_OTHER_KEY".into()),
        )
        .unwrap();
        assert_eq!(
            transport::test_connection(
                &f.db,
                &f.vault,
                &c.id,
                true,
                &mut api,
                &CancellationToken::new()
            )
            .unwrap_err(),
            "API_ADAPTER_NOT_IMPLEMENTED"
        );
    }
    assert!(api.calls.is_empty());
    assert_eq!(config::list_configs(&f.db).unwrap().len(), 7);
}
#[test]
fn config_validation_connection_approval_and_delete() {
    let f = Fixture::new();
    let mut c = f.config.clone();
    for base in [
        "http://api.deepseek.com/v1",
        "https://u:p@api.deepseek.com/v1",
        "https://api.deepseek.com/v1?token=x",
        "https://console.deepseek.com/v1",
    ] {
        c.base_url = base.into();
        assert!(c.validate().is_err());
    }
    let mut api = FakeApi::default();
    assert_eq!(
        transport::test_connection(
            &f.db,
            &f.vault,
            &f.config.id,
            false,
            &mut api,
            &CancellationToken::new()
        )
        .unwrap_err(),
        "API_NETWORK_APPROVAL_REQUIRED"
    );
    assert!(api.calls.is_empty());
    let result = transport::test_connection(
        &f.db,
        &f.vault,
        &f.config.id,
        true,
        &mut api,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(result.reachable && !result.development_verified);
    config::delete_key(&f.db, &f.vault, &f.config.id).unwrap();
    assert!(!config::list_configs(&f.db).unwrap()[0].has_key);
    assert_eq!(
        transport::test_connection(
            &f.db,
            &f.vault,
            &f.config.id,
            true,
            &mut api,
            &CancellationToken::new()
        )
        .unwrap_err(),
        "API_KEY_MISSING"
    );
}
#[test]
fn selected_secret_in_input_rejects_before_persistence_and_endpoint_cannot_reuse_key() {
    let f = Fixture::new();
    std::fs::write(f.root.join("note.txt"), "SYNTHETIC_SECRET_VALUE_12345678").unwrap();
    assert!(prepare(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into()],
        vec![f.test()]
    )
    .is_err());
    assert_eq!(
        f.db.conn()
            .query_row("SELECT count(*) FROM api_task_runs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let mut c = f.config.clone();
    c.provider = Provider::OpenaiCompatible;
    c.base_url = "https://example.com/v1".into();
    assert_eq!(
        config::save_config(&f.db, &f.vault, c, None).unwrap_err(),
        "API_NEW_ENDPOINT_REQUIRES_NEW_KEY"
    );
    let mut c = f.config.clone();
    c.model = "SYNTHETIC_SECRET_VALUE_12345678".into();
    assert!(config::save_config(&f.db, &f.vault, c, None).is_err());
    assert_eq!(config::list_configs(&f.db).unwrap()[0].config, f.config);
}
#[test]
fn cancelled_before_start_does_not_consume_approval_or_send() {
    let f = Fixture::new();
    let p = f.approved();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut api = FakeApi::default();
    let mut tests = FakeTests {
        state: "PASS",
        calls: 0,
    };
    assert_eq!(
        execute(&f.db, &p.id, &f.vault, &mut api, &mut tests, &cancel).unwrap_err(),
        "API_CANCELLED_BEFORE_START"
    );
    assert!(api.calls.is_empty());
    assert_eq!(status(&f.db, &p.id).unwrap()["phase"], "APPROVED");
}

#[test]
fn two_approved_files_change_and_other_files_remain_untouched() {
    let f = Fixture::new();
    std::fs::write(f.root.join("second.txt"), b"Alpha\r\n").unwrap();
    std::fs::write(f.root.join("untouched.txt"), b"USER WORK").unwrap();
    let p = prepare(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into(), "second.txt".into()],
        vec![f.test()],
    )
    .unwrap();
    approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
    let (r, api, _) = execute_case(&f, &p, "multi", "PASS");
    assert_eq!(r["phase"], "AWAITING_USER");
    assert_eq!(api.calls.len(), 2);
    assert_eq!(
        std::fs::read(f.root.join("second.txt")).unwrap(),
        b"beta\r\n"
    );
    assert_eq!(
        std::fs::read(f.root.join("untouched.txt")).unwrap(),
        b"USER WORK"
    );
}
