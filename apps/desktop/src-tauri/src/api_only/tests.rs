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
            recipe: "content".into(),
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
            counts: None,
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
// Offline scripted transport: real persistence/execute code, no sockets or real credentials.
struct DiagnosedPort<P> { inner: P, timeout: u8, calls: usize }
impl<P: ApiPort> ApiPort for DiagnosedPort<P> {
    fn send(&mut self, r: &transport::WireRequest, key: &SecretLease, c: &CancellationToken) -> Result<Vec<u8>> {
        self.inner.send(r, key, c)
    }
    fn send_diagnosed(&mut self, r: &transport::WireRequest, key: &SecretLease,
        c: &CancellationToken, observe: &mut dyn FnMut(&transport::TransportFacts)) -> Result<Vec<u8>> {
        self.calls += 1;
        let mut f = transport::TransportFacts::begin();
        observe(&f);
        if self.timeout != 1 {
            f.headers_received = true; f.http_status = Some(200); observe(&f);
        }
        let result = if self.timeout > 0 {
            if self.timeout == 2 { f.body_bytes_read = 17; }
            Err("API_TIMEOUT_UNKNOWN")
        } else {
            let out = self.inner.send(r, key, c);
            if let Ok(b) = &out { f.body_bytes_read = b.len() as u64; f.body_complete = true; }
            out
        };
        f.finish(&result, Duration::from_millis(if self.timeout > 0 { 60000 } else { 7 }));
        observe(&f);
        result
    }
}
#[test]
fn transport_observations_persist_preheader_and_partial_body_timeout() {
    for timeout in [1, 2] {
        let f = Fixture::new(); let p = f.approved();
        let before = std::fs::read(f.root.join("note.txt")).unwrap();
        let mut api = DiagnosedPort { inner: FakeApi::default(), timeout, calls: 0 };
        let mut tests = FakeTests {state:"PASS",calls:0};
        let out = execute(&f.db,&p.id,&f.vault,&mut api,&mut tests,&CancellationToken::new()).unwrap();
        assert_eq!(out["phase"],"UNKNOWN");
        assert_eq!(out["summary"]["error_code"],"API_TIMEOUT_UNKNOWN");
        assert_eq!(out["summary"]["stopped_phase"],"DEVELOP_STARTED");
        let facts=&out["summary"]["develop_transport"];
        assert_eq!(facts["headers_received"],timeout==2);
        assert_eq!(facts["http_status"],if timeout==2 {json!(200)} else {Value::Null});
        assert_eq!(facts["body_complete"],false);
        assert_eq!(facts["body_bytes_read"],if timeout==2 {17} else {0});
        assert_eq!(facts["elapsed_ms"],60000);
        assert_eq!(facts["error_category"],"TIMEOUT_UNKNOWN");
        chrono::DateTime::parse_from_rfc3339(facts["attempt_started_utc"].as_str().unwrap()).unwrap();
        assert!(out["summary"].get("changes").is_none());
        assert_eq!((api.calls,tests.calls),(1,0));
        assert_eq!(std::fs::read(f.root.join("note.txt")).unwrap(),before);
        assert_eq!(status(&f.db,&p.id).unwrap(),out);
        assert!(execute(&f.db,&p.id,&f.vault,&mut api,&mut tests,&CancellationToken::new()).is_err());
        assert_eq!(api.calls,1);
        assert_eq!(f.db.task(&f.task).unwrap().state,TaskState::Confirmed);
        let text=encode(facts).unwrap();
        assert!(!text.contains("SYNTHETIC_SECRET_VALUE_12345678"));
        assert_eq!(facts.as_object().unwrap().len(),7);
    }
}
#[test]
fn transport_observations_complete_and_no_sensitive_diagnostics() {
    let f=Fixture::new();let p=f.approved();
    let mut api=DiagnosedPort {inner:FakeApi {root:f.root.clone(),..Default::default()},timeout:0,calls:0};
    let mut tests=FakeTests {state:"PASS",calls:0};
    let out=execute(&f.db,&p.id,&f.vault,&mut api,&mut tests,&CancellationToken::new()).unwrap();
    assert_eq!(out["phase"],"AWAITING_USER");
    for field in ["develop_transport","review_transport"] {
        let facts=&out["summary"][field];
        assert_eq!(facts["body_complete"],true);assert_eq!(facts["http_status"],200);
        assert!(facts["body_bytes_read"].as_u64().unwrap()>0);assert_eq!(facts["error_category"],"NONE");
        let text=encode(facts).unwrap();
        for secret in ["SYNTHETIC_SECRET_VALUE_12345678","Approved word change","choices","Authorization"] {assert!(!text.contains(secret));}
    }
    assert_eq!((api.calls,tests.calls),(2,1));
    let mut facts=transport::TransportFacts::begin();
    facts.finish(&Err("SYNTHETIC_SECRET_VALUE_12345678 raw private error"),Duration::from_millis(1));
    assert_eq!(serde_json::to_value(&facts).unwrap()["error_category"],"OTHER_REJECTED");
    assert!(!encode(&facts).unwrap().contains("SYNTHETIC"));
}
#[test]
fn transport_diagnostic_storage_failure_preserves_timeout() {
    let f=Fixture::new();let p=f.approved();
    f.db.conn().execute_batch("CREATE TEMP TRIGGER diagnostic_failure BEFORE UPDATE ON api_task_runs WHEN NEW.phase=OLD.phase AND NEW.summary_json LIKE '%develop_transport%' BEGIN SELECT RAISE(FAIL,'synthetic diagnostic storage failure'); END;").unwrap();
    let mut api=DiagnosedPort {inner:FakeApi::default(),timeout:2,calls:0};
    let mut tests=FakeTests {state:"PASS",calls:0};
    let out=execute(&f.db,&p.id,&f.vault,&mut api,&mut tests,&CancellationToken::new()).unwrap();
    assert_eq!(out["phase"],"UNKNOWN");assert_eq!(out["summary"]["error_code"],"API_TIMEOUT_UNKNOWN");
    assert_eq!(out["summary"]["transport_recording"],"INCOMPLETE");assert_eq!((api.calls,tests.calls),(1,0));
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
// API-2 fixtures only; never reads an OS credential or contacts a provider.
struct LoopApi {
    calls: usize,
    reviews: usize,
    fail_second: bool,
}
impl ApiPort for LoopApi {
    fn send(
        &mut self,
        r: &transport::WireRequest,
        _: &SecretLease,
        _: &CancellationToken,
    ) -> Result<Vec<u8>> {
        self.calls += 1;
        let b: Value = decode(&r.body, 131072)?;
        let i: Value = decode(
            b["messages"][1]["content"].as_str().unwrap().as_bytes(),
            131072,
        )?;
        assert_eq!(b["messages"].as_array().unwrap().len(), 2);
        assert!(b.get("tools").is_none());
        let v = if i.get("files").is_some() {
            let before = i["files"][0]["before"].as_str().unwrap();
            let old = if before.contains("Alpha") {
                "Alpha"
            } else {
                "beta"
            };
            json!({"schema":"nova.api.edits/1","edits":[{"schema":"nova.relay.proposal/1","target_path":"note.txt","old_text":old,"new_text":if old=="Alpha"{"beta"}else{"gamma"},"rationale":"Approved repair"}]})
        } else {
            self.reviews += 1;
            json!({"schema":"nova.relay.review/1","base_sha256":i["base_sha256"],"candidate_sha256":i["candidate_sha256"],"verdict":if self.reviews==1||self.fail_second{"CHANGES_REQUIRED"}else{"PASS"},"issues":[],"unknowns":[]})
        };
        Ok(serde_json::to_vec(&json!({"model":b["model"],"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":encode(&v)?}}]})).unwrap())
    }
}
#[test]
fn preauthorized_repair_persists_once_then_user_accepts_through_gate() {
    let f = Fixture::new();
    let p = prepare_scoped(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into()],
        vec![f.test()],
        true,
        None,
    )
    .unwrap();
    approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
    let mut api = LoopApi {
        calls: 0,
        reviews: 0,
        fail_second: false,
    };
    let mut tests = FakeTests {
        state: "PASS",
        calls: 0,
    };
    let r = task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut api,
        &mut tests,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(r["phase"], "AWAITING_USER");
    assert_eq!(api.calls, 4);
    assert_eq!(tests.calls, 2);
    assert_eq!(
        f.db.task(&f.task).unwrap().state,
        TaskState::AwaitingUserAcceptance
    );
    assert!(f
        .db
        .transition_task(
            &f.task,
            TaskState::Completed,
            Actor::Nova,
            None,
            None,
            false,
            None
        )
        .is_err());
    assert!(task_loop::repair(
        &f.db,
        &f.vault,
        &p.id,
        vec!["note.txt".into()],
        vec![f.test()],
        &f.config.id
    )
    .is_err());
    assert!(f
        .db
        .conn()
        .execute("DELETE FROM api_task_runs", [])
        .is_err());
    assert!(f
        .db
        .conn()
        .execute("DELETE FROM api_task_scopes", [])
        .is_err());
    task_loop::user_decision(
        &f.db,
        r["run_id"].as_str().unwrap(),
        true,
        "verified by user",
    )
    .unwrap();
    assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::Completed);
    assert_eq!(
        task_loop::report(&f.db, &f.task).unwrap()["repair_count"],
        1
    );
}
#[test]
fn no_grant_requires_new_approval_and_scope_expansion_is_not_inherited() {
    let f = Fixture::new();
    let p = f.approved();
    let mut a = LoopApi {
        calls: 0,
        reviews: 0,
        fail_second: false,
    };
    let mut t = FakeTests {
        state: "PASS",
        calls: 0,
    };
    let r = task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(r["phase"], "NEEDS_REMEDIATION");
    assert_eq!(a.calls, 2);
    std::fs::write(f.root.join("extra.txt"), "extra").unwrap();
    let next = task_loop::repair(
        &f.db,
        &f.vault,
        &p.id,
        vec!["note.txt".into(), "extra.txt".into()],
        vec![f.test()],
        &f.config.id,
    )
    .unwrap();
    assert!(execute(
        &f.db,
        &next.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new()
    )
    .is_err());
    assert_eq!(a.calls, 2);
    assert_eq!(status(&f.db, &next.id).unwrap()["phase"], "PREPARED");
}
#[test]
fn repair_failure_stops_and_unknown_cannot_get_new_plan() {
    let f = Fixture::new();
    let p = prepare_scoped(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into()],
        vec![f.test()],
        true,
        None,
    )
    .unwrap();
    approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
    let mut a = LoopApi {
        calls: 0,
        reviews: 0,
        fail_second: true,
    };
    let mut t = FakeTests {
        state: "PASS",
        calls: 0,
    };
    let r = task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(r["phase"], "NEEDS_REMEDIATION");
    assert_eq!(a.calls, 4);
    assert!(task_loop::repair(
        &f.db,
        &f.vault,
        r["run_id"].as_str().unwrap(),
        vec!["note.txt".into()],
        vec![f.test()],
        &f.config.id
    )
    .is_err());
    let f = Fixture::new();
    let p = f.approved();
    let (r, _, _) = execute_case(&f, &p, "timeout", "PASS");
    assert_eq!(r["phase"], "UNKNOWN");
    assert!(task_loop::repair(
        &f.db,
        &f.vault,
        &p.id,
        vec!["note.txt".into()],
        vec![f.test()],
        &f.config.id
    )
    .is_err());
}
#[test]
fn user_feedback_and_post_delivery_drift_do_not_complete() {
    let f = Fixture::new();
    let p = f.approved();
    let mut a = FakeApi {
        root: f.root.clone(),
        ..Default::default()
    };
    let mut t = FakeTests {
        state: "PASS",
        calls: 0,
    };
    let r = task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(r["phase"], "AWAITING_USER");
    task_loop::user_decision(&f.db, &p.id, false, "please adjust").unwrap();
    assert_eq!(
        f.db.task(&f.task).unwrap().state,
        TaskState::NeedsUserAction
    );
    std::fs::write(f.root.join("note.txt"), "beta USER EDIT\r\n").unwrap();
    let next = task_loop::repair(
        &f.db,
        &f.vault,
        &p.id,
        vec!["note.txt".into()],
        vec![f.test()],
        &f.config.id,
    )
    .unwrap();
    assert_eq!(next.files[0].before, "beta USER EDIT\r\n");
    assert!(!next.allow_repair);
    assert_eq!(next.prior_issues["user_feedback"], "please adjust");
    let f = Fixture::new();
    let p = f.approved();
    let mut a = FakeApi {
        root: f.root.clone(),
        ..Default::default()
    };
    task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new(),
    )
    .unwrap();
    std::fs::write(f.root.join("note.txt"), "USER EDIT").unwrap();
    assert!(task_loop::user_decision(&f.db, &p.id, true, "accept").is_err());
    assert_eq!(
        f.db.task(&f.task).unwrap().state,
        TaskState::AwaitingUserAcceptance
    );
}
#[test]
fn project_unittest_real_success_failure_timeout_and_environment_is_empty_of_secrets() {
    let f = Fixture::new();
    std::fs::create_dir(f.root.join("tests")).unwrap();
    let path = f.root.join("tests/test_canary.py");
    let spec = FixedTest {
        recipe: "python_unittest".into(),
        ..f.test()
    };
    let original = vec![FileSnapshot {
        path: "note.txt".into(),
        identity: "unused".into(),
        before: "unused".into(),
        sha256: "unused".into(),
    }];
    let t = tool_snapshot(&spec, &f.root, &original).unwrap();
    assert!(t
        .environment
        .keys()
        .all(|k| matches!(k.as_str(), "SystemRoot" | "WINDIR")));
    for (source,expected) in [("import unittest,os\nclass T(unittest.TestCase):\n def test_env(self):\n  self.assertNotIn('NOVA_SYNTHETIC_PROVIDER_KEY',os.environ)\n  self.assertNotIn('PATH',os.environ)\n", "PASS"),("import unittest\nclass T(unittest.TestCase):\n def test_bad(self): self.fail('synthetic failure')\n","FAIL"),("import time\ntime.sleep(60)\n","UNKNOWN")] {
  std::fs::write(&path,source).unwrap();let ev=FixedTestRunner.run(&t,&f.root,&CancellationToken::new()).unwrap();assert_eq!(ev.status,expected);assert!(!ev.stdout.contains("SYNTHETIC_SECRET"));
 }
}
#[test]
fn credential_rotation_cleanup_failure_is_recorded_without_secret() {
    struct FailingDelete(MemoryCredentialStore);
    impl CredentialStore for FailingDelete {
        fn save_api_key(&self, s: &str, k: &str) -> crate::error::NovaResult<()> {
            self.0.save_api_key(s, k)
        }
        fn read_api_key(&self, s: &str) -> crate::error::NovaResult<Option<String>> {
            self.0.read_api_key(s)
        }
        fn delete_api_key(&self, _: &str) -> crate::error::NovaResult<()> {
            Err(crate::error::NovaError::Credential(
                "synthetic failure".into(),
            ))
        }
    }
    let f = Fixture::new();
    let v = FailingDelete(MemoryCredentialStore::default());
    config::save_config(
        &f.db,
        &v,
        f.config.clone(),
        Some("NEW_SYNTHETIC_SECRET".into()),
    )
    .unwrap();
    let status = config::cleanup_status(&f.db).unwrap();
    assert_eq!(status["status"], "CLEANUP_PENDING");
    assert_eq!(status["pending"], 1);
    assert!(!encode(&status).unwrap().contains("SECRET"));
    assert_eq!(
        config::delete_key(&f.db, &v, &f.config.id).unwrap_err(),
        "API_CREDENTIAL_DELETE_FAILED"
    );
}
#[test]
fn nova_node_recipe_uses_fixed_argv_cwd_and_explicit_environment_on_synthetic_project() {
    let f = Fixture::new();
    let program = std::env::var_os("PATH")
        .and_then(|v| {
            std::env::split_paths(&v)
                .map(|p| p.join("node.exe"))
                .find(|p| p.is_file())
        })
        .expect("installed Node required for Nova recipe test");
    let module = f.root.join("apps/desktop/node_modules/vitest");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("vitest.mjs"),"import assert from 'node:assert/strict'; assert.equal(process.env.PATH,undefined); assert.equal(process.argv[2],'run'); assert.equal(process.argv[3],'src/apiTaskPanel.interaction.test.ts'); console.log('SYNTHETIC_NODE_PASS');").unwrap();
    let spec = FixedTest {
        program,
        recipe: "nova_vitest_api2".into(),
        ..f.test()
    };
    let files = vec![FileSnapshot {
        path: "note.txt".into(),
        identity: "unused".into(),
        before: "unused".into(),
        sha256: "unused".into(),
    }];
    let t = tool_snapshot(&spec, &f.root, &files).unwrap();
    assert_eq!(t.cwd, f.root.join("apps/desktop"));
    let ev = FixedTestRunner
        .run(&t, &f.root, &CancellationToken::new())
        .unwrap();
    assert_eq!(ev.status, "PASS");
    assert!(ev.stdout.contains("SYNTHETIC_NODE_PASS"));
}
#[test]
fn missing_test_evidence_and_receipt_binding_cannot_pass_existing_completion_gate() {
    let f = Fixture::new();
    let p = f.approved();
    let mut api = FakeApi {
        root: f.root.clone(),
        ..Default::default()
    };
    let mut tests = FakeTests {
        state: "PASS",
        calls: 0,
    };
    task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut api,
        &mut tests,
        &CancellationToken::new(),
    )
    .unwrap();
    let (_, _, _, mut summary) = load_plan(&f.db, &p.id).unwrap();
    summary["tests"] = json!([]);
    f.db.conn()
        .execute(
            "UPDATE api_task_runs SET summary_json=?1 WHERE id=?2",
            rusqlite::params![encode(&summary).unwrap(), p.id],
        )
        .unwrap();
    assert!(f
        .db
        .evaluate_acceptance_submission(&f.db.task(&f.task).unwrap())
        .unwrap()
        .is_err());
    assert!(task_loop::user_decision(&f.db, &p.id, true, "must not accept").is_err());
    assert!(f.db.user_acceptance_for_task(&f.task).unwrap().is_none());
}
#[test]
fn failed_config_cas_cleanup_is_recorded_and_delete_cas_never_claims_success() {
    struct FailDelete(MemoryCredentialStore);
    impl CredentialStore for FailDelete {
        fn save_api_key(&self, s: &str, k: &str) -> crate::error::NovaResult<()> {
            self.0.save_api_key(s, k)
        }
        fn read_api_key(&self, s: &str) -> crate::error::NovaResult<Option<String>> {
            self.0.read_api_key(s)
        }
        fn delete_api_key(&self, _: &str) -> crate::error::NovaResult<()> {
            Err(crate::error::NovaError::Credential("synthetic".into()))
        }
    }
    let f = Fixture::new();
    f.db.conn().execute_batch("CREATE TRIGGER synthetic_conflict BEFORE UPDATE ON api_model_configs BEGIN SELECT RAISE(IGNORE); END;").unwrap();
    let v = FailDelete(MemoryCredentialStore::default());
    assert_eq!(
        config::save_config(
            &f.db,
            &v,
            f.config.clone(),
            Some("SYNTHETIC_NEW_KEY".into())
        )
        .unwrap_err(),
        "API_CONFIG_CONFLICT"
    );
    assert_eq!(config::cleanup_status(&f.db).unwrap()["pending"], 1);
    assert_eq!(
        config::delete_key(&f.db, &f.vault, &f.config.id).unwrap_err(),
        "API_KEY_DELETE_CONFIG_CONFLICT"
    );
}
#[test]
fn feedback_key_echo_rejected_before_acceptance_record_and_config_drift_can_be_declined() {
    let f = Fixture::new();
    let p = f.approved();
    let mut a = FakeApi {
        root: f.root.clone(),
        ..Default::default()
    };
    let mut t = FakeTests {
        state: "PASS",
        calls: 0,
    };
    task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(task_loop::user_decision_with_vault(
        &f.db,
        &f.vault,
        &p.id,
        true,
        "SYNTHETIC_SECRET_VALUE_12345678"
    )
    .is_err());
    assert!(f.db.user_acceptance_for_task(&f.task).unwrap().is_none());
    config::save_config(&f.db, &f.vault, f.config.clone(), None).unwrap();
    assert!(task_loop::user_decision_with_vault(&f.db, &f.vault, &p.id, true, "accept").is_err());
    task_loop::user_decision_with_vault(
        &f.db,
        &f.vault,
        &p.id,
        false,
        "update configuration then reapprove",
    )
    .unwrap();
    assert_eq!(
        f.db.task(&f.task).unwrap().state,
        TaskState::NeedsUserAction
    );
}
#[test]
fn pending_cancellation_releases_serial_slot_without_erasing_approval_scope() {
    let f = Fixture::new();
    let p = f.approved();
    task_loop::cancel_pending(&f.db, &f.task).unwrap();
    assert_eq!(status(&f.db, &p.id).unwrap()["phase"], "CANCELLED");
    let mut a = FakeApi::default();
    let mut t = FakeTests {
        state: "PASS",
        calls: 0,
    };
    assert!(execute(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new()
    )
    .is_err());
    assert!(a.calls.is_empty());
    assert_eq!(
        f.db.conn()
            .query_row(
                "SELECT count(*) FROM api_task_runs WHERE phase NOT IN ('CANCELLED')",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        f.db.conn()
            .query_row("SELECT count(*) FROM api_task_scopes", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn publication_failure_is_unknown_and_frozen_legacy_task_never_prepares() {
    let f = Fixture::new();
    let p = f.approved();
    f.db.conn().execute_batch("CREATE TRIGGER synthetic_publish_failure BEFORE UPDATE OF state ON tasks WHEN NEW.state='queued' BEGIN SELECT RAISE(ABORT,'synthetic publication failure'); END;").unwrap();
    let mut a = FakeApi {
        root: f.root.clone(),
        ..Default::default()
    };
    let mut t = FakeTests {
        state: "PASS",
        calls: 0,
    };
    let r = task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(r["phase"], "UNKNOWN");
    assert_eq!(a.calls.len(), 2);
    assert_eq!(f.db.task(&f.task).unwrap().state, TaskState::Confirmed);
    assert!(execute(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new()
    )
    .is_err());
    assert_eq!(a.calls.len(), 2);
    let f = Fixture::new();
    f.db.conn()
        .execute(
            "UPDATE tasks SET legacy_classification='unknown_insufficient' WHERE id=?1",
            [&f.task],
        )
        .unwrap();
    assert_eq!(
        prepare(
            &f.db,
            &f.vault,
            &f.task,
            &f.config.id,
            vec!["note.txt".into()],
            vec![f.test()]
        )
        .unwrap_err(),
        "API_LEGACY_TASK_FROZEN"
    );
}
#[test]
fn user_feedback_can_consume_existing_grant_but_drift_cannot_inherit_it() {
    for drift in [false, true] {
        let f = Fixture::new();
        let p = prepare_scoped(
            &f.db,
            &f.vault,
            &f.task,
            &f.config.id,
            vec!["note.txt".into()],
            vec![f.test()],
            true,
            None,
        )
        .unwrap();
        approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
        let mut first = FakeApi {
            root: f.root.clone(),
            ..Default::default()
        };
        let mut tests = FakeTests {
            state: "PASS",
            calls: 0,
        };
        task_loop::execute_loop(
            &f.db,
            &p.id,
            &f.vault,
            &mut first,
            &mut tests,
            &CancellationToken::new(),
        )
        .unwrap();
        task_loop::user_decision_with_vault(&f.db, &f.vault, &p.id, false, "repair within scope")
            .unwrap();
        if drift {
            std::fs::write(f.root.join("note.txt"), "USER EDIT").unwrap();
        }
        let mut second = LoopApi {
            calls: 0,
            reviews: 1,
            fail_second: false,
        };
        let r = task_loop::execute_loop(
            &f.db,
            &p.id,
            &f.vault,
            &mut second,
            &mut tests,
            &CancellationToken::new(),
        );
        if drift {
            assert!(r.is_err());
            assert_eq!(second.calls, 0);
            assert_eq!(
                std::fs::read_to_string(f.root.join("note.txt")).unwrap(),
                "USER EDIT"
            );
        } else {
            assert_eq!(r.unwrap()["phase"], "AWAITING_USER");
            assert_eq!(second.calls, 2);
            assert_eq!(
                task_loop::report(&f.db, &f.task).unwrap()["repair_count"],
                1
            );
        }
    }
}
#[test]
fn selected_project_root_drift_rejects_before_send() {
    let f = Fixture::new();
    let p = f.approved();
    let other = f.root.join("different-root");
    std::fs::create_dir(&other).unwrap();
    f.db.conn()
        .execute(
            "UPDATE nova_projects SET local_path=?1 WHERE id=?2",
            rusqlite::params![other.to_string_lossy(), p.project_id],
        )
        .unwrap();
    let mut a = FakeApi::default();
    let mut t = FakeTests {
        state: "PASS",
        calls: 0,
    };
    assert_eq!(
        execute(
            &f.db,
            &p.id,
            &f.vault,
            &mut a,
            &mut t,
            &CancellationToken::new()
        )
        .unwrap_err(),
        "API_PROJECT_ROOT_DRIFT"
    );
    assert!(a.calls.is_empty());
    assert_eq!(
        std::fs::read_to_string(f.root.join("note.txt")).unwrap(),
        p.files[0].before
    );
}
#[test]
fn full_loop_edits_python_function_runs_real_unittest_reviews_and_waits_for_user() {
    let f = Fixture::new();
    std::fs::write(
        f.root.join("subject.py"),
        "def normalize(value):\n    return value\n",
    )
    .unwrap();
    std::fs::create_dir(f.root.join("tests")).unwrap();
    std::fs::write(f.root.join("tests/test_subject.py"),"import unittest,importlib.util\ns=importlib.util.spec_from_file_location('subject','subject.py')\nm=importlib.util.module_from_spec(s)\ns.loader.exec_module(m)\nclass T(unittest.TestCase):\n def test_behavior(self):\n  self.assertEqual(m.normalize(' Alpha '),'alpha')\n  self.assertEqual(m.normalize(' Ä '),'ä')\n").unwrap();
    let old = f.db.task(&f.task).unwrap();
    let task =
        f.db.create_task(
            &CreateTaskRequest {
                project_id: old.project_id,
                title: "Normalize function".into(),
                goal: Some("Normalize input by trimming and lowercasing".into()),
                in_scope: Some("subject.py".into()),
                not_in_scope: Some("No other source edits".into()),
                done_criteria: Some("Python unittest passes".into()),
                ai_kind: None,
            },
            Actor::User,
        )
        .unwrap();
    for state in [TaskState::AwaitingConfirmation, TaskState::Confirmed] {
        f.db.transition_task(&task.id, state, Actor::User, None, None, false, None)
            .unwrap();
    }
    let test = FixedTest {
        file: "subject.py".into(),
        recipe: "python_unittest".into(),
        ..f.test()
    };
    let p = prepare_scoped(
        &f.db,
        &f.vault,
        &task.id,
        &f.config.id,
        vec!["subject.py".into()],
        vec![test],
        false,
        None,
    )
    .unwrap();
    approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
    struct FunctionalApi(usize);
    impl ApiPort for FunctionalApi {
        fn send(
            &mut self,
            r: &transport::WireRequest,
            _: &SecretLease,
            _: &CancellationToken,
        ) -> Result<Vec<u8>> {
            self.0 += 1;
            let b: Value = decode(&r.body, 131072)?;
            let input: Value = decode(
                b["messages"][1]["content"].as_str().unwrap().as_bytes(),
                131072,
            )?;
            let v = if self.0 == 1 {
                json!({"schema":"nova.api.edits/1","edits":[{"schema":"nova.relay.proposal/1","target_path":"subject.py","old_text":"return value","new_text":"return value.strip().lower()","rationale":"Implement approved function behavior"}]})
            } else {
                assert_eq!(input["actual_tests"][0]["status"], "PASS");
                assert_eq!(input["actual_tests"][0]["exit_code"], 0);
                json!({"schema":"nova.relay.review/1","base_sha256":input["base_sha256"],"candidate_sha256":input["candidate_sha256"],"verdict":"PASS","issues":[],"unknowns":[]})
            };
            Ok(serde_json::to_vec(&json!({"model":b["model"],"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":encode(&v)?}}]})).unwrap())
        }
    }
    let mut api = FunctionalApi(0);
    let result = task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut api,
        &mut FixedTestRunner,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(api.0, 2);
    assert_eq!(result["phase"], "AWAITING_USER");
    assert_eq!(
        f.db.task(&task.id).unwrap().state,
        TaskState::AwaitingUserAcceptance
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join("subject.py")).unwrap(),
        "def normalize(value):\n    return value.strip().lower()\n"
    );
    assert!(!f.root.join("__pycache__").exists());
    task_loop::user_decision_with_vault(&f.db, &f.vault, &p.id, true, "synthetic user acceptance")
        .unwrap();
    assert_eq!(f.db.task(&task.id).unwrap().state, TaskState::Completed);
}
#[test]
fn disk_reopen_keeps_repair_limit_and_unknown_without_resending() {
    for unknown in [false, true] {
        let f = Fixture::new();
        let p = prepare_scoped(
            &f.db,
            &f.vault,
            &f.task,
            &f.config.id,
            vec!["note.txt".into()],
            vec![f.test()],
            true,
            None,
        )
        .unwrap();
        approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
        let mut tests = FakeTests {
            state: "PASS",
            calls: 0,
        };
        let result = if unknown {
            let (r, _, _) = execute_case(&f, &p, "timeout", "PASS");
            r
        } else {
            let mut a = LoopApi {
                calls: 0,
                reviews: 0,
                fail_second: false,
            };
            let r = task_loop::execute_loop(
                &f.db,
                &p.id,
                &f.vault,
                &mut a,
                &mut tests,
                &CancellationToken::new(),
            )
            .unwrap();
            assert_eq!(a.calls, 4);
            r
        };
        let path = f.root.with_extension("sqlite3");
        f.db.conn()
            .execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])
            .unwrap();
        let reopened = Database::open(&path).unwrap();
        reopened.migrate().unwrap();
        assert_eq!(
            task_loop::report(&reopened, &f.task).unwrap()["repair_count"],
            if unknown { 0 } else { 1 }
        );
        let mut a = FakeApi::default();
        assert!(task_loop::execute_loop(
            &reopened,
            result["run_id"].as_str().unwrap(),
            &f.vault,
            &mut a,
            &mut tests,
            &CancellationToken::new()
        )
        .is_err());
        assert!(a.calls.is_empty());
        assert!(task_loop::repair(
            &reopened,
            &f.vault,
            &p.id,
            vec!["note.txt".into()],
            vec![f.test()],
            &f.config.id
        )
        .is_err());
        assert!(reopened
            .conn()
            .execute("DELETE FROM api_task_runs", [])
            .is_err());
    }
}
#[test]
fn p1_inherited_deadline_crossing_stops_before_write() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_CLOCK.with(|c| c.set(None));
        }
    }
    let _reset = Reset;
    TEST_CLOCK.with(|c| c.set(Some(10000)));
    let f = Fixture::new();
    let p = prepare_scoped(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into()],
        vec![f.test()],
        true,
        None,
    )
    .unwrap();
    approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
    let mut a = LoopApi {
        calls: 0,
        reviews: 0,
        fail_second: false,
    };
    let mut t = FakeTests {
        state: "PASS",
        calls: 0,
    };
    execute(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut t,
        &CancellationToken::new(),
    )
    .unwrap();
    TEST_CLOCK.with(|c| c.set(Some(p.expires_utc - 1)));
    let next = task_loop::inherit_repair(&f.db, &f.vault, &p.id, &p).unwrap();
    assert_eq!(next.expires_utc, p.expires_utc);
    struct Cross {
        inner: LoopApi,
        deadline: i64,
    }
    impl ApiPort for Cross {
        fn send(
            &mut self,
            r: &transport::WireRequest,
            k: &SecretLease,
            c: &CancellationToken,
        ) -> Result<Vec<u8>> {
            let out = self.inner.send(r, k, c);
            TEST_CLOCK.with(|v| v.set(Some(self.deadline)));
            out
        }
    }
    let before = std::fs::read(f.root.join("note.txt")).unwrap();
    let mut crossing = Cross {
        inner: LoopApi {
            calls: 0,
            reviews: 1,
            fail_second: false,
        },
        deadline: p.expires_utc,
    };
    let mut tests = FakeTests {
        state: "PASS",
        calls: 0,
    };
    let r = execute(
        &f.db,
        &next.id,
        &f.vault,
        &mut crossing,
        &mut tests,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_ne!(r["phase"], "AWAITING_USER");
    assert_eq!(crossing.inner.calls, 1);
    assert_eq!(tests.calls, 0);
    assert_eq!(std::fs::read(f.root.join("note.txt")).unwrap(), before);
    assert!(task_loop::inherit_repair(&f.db, &f.vault, &p.id, &p).is_err());
}
#[test]
fn p1_manual_deadline_is_new_explicit_approval() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_CLOCK.with(|c| c.set(None));
        }
    }
    let _reset = Reset;
    TEST_CLOCK.with(|c| c.set(Some(20000)));
    let f = Fixture::new();
    let p = f.approved();
    execute_case(&f, &p, "changes", "PASS");
    TEST_CLOCK.with(|c| c.set(Some(p.expires_utc + 1)));
    let child = task_loop::repair(
        &f.db,
        &f.vault,
        &p.id,
        vec!["note.txt".into()],
        vec![f.test()],
        &f.config.id,
    )
    .unwrap();
    assert!(child.expires_utc > p.expires_utc);
    assert_eq!(status(&f.db, &child.id).unwrap()["phase"], "PREPARED");
}
#[test]
fn p1_runtime_suite_counts_empty_mismatch_skip_failure_success() {
    let f = Fixture::new();
    std::fs::create_dir(f.root.join("tests")).unwrap();
    let spec = FixedTest {
        recipe: "python_unittest".into(),
        ..f.test()
    };
    let files = vec![FileSnapshot {
        path: "note.txt".into(),
        identity: "unused".into(),
        before: "unused".into(),
        sha256: "unused".into(),
    }];
    let t = tool_snapshot(&spec, &f.root, &files).unwrap();
    for (name,source,pass) in [("other.py","import unittest\nclass T(unittest.TestCase):\n def test_a(self): pass\n",false),("test_a.py","import unittest\nclass T(unittest.TestCase):\n def test_a(self): pass\n",true),("test_a.py","import unittest\n@unittest.skip('synthetic')\nclass T(unittest.TestCase):\n def test_a(self): pass\n",false),("test_a.py","import unittest\nclass T(unittest.TestCase):\n def test_a(self): self.fail('synthetic')\n",false)]{std::fs::write(f.root.join("tests").join(name),source).unwrap();let e=FixedTestRunner.run(&t,&f.root,&CancellationToken::new()).unwrap();assert_eq!(e.status=="PASS",pass);assert!(e.counts.is_some());}
    assert!(recipes::counts("OK").is_none());
    assert!(recipes::counts("NOVA_UNIT_RESULT {}\n").is_none());
}
#[test]
fn p1_empty_suite_never_publishes_or_completes() {
    let f = Fixture::new();
    std::fs::create_dir(f.root.join("tests")).unwrap();
    let spec = FixedTest {
        recipe: "python_unittest".into(),
        ..f.test()
    };
    let p = prepare_scoped(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into()],
        vec![spec],
        false,
        None,
    )
    .unwrap();
    approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
    let mut api = FakeApi {
        root: f.root.clone(),
        ..Default::default()
    };
    let r = task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut api,
        &mut FixedTestRunner,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_ne!(r["phase"], "AWAITING_USER");
    assert!(task_loop::user_decision_with_vault(&f.db, &f.vault, &p.id, true, "accept").is_err());
}
#[test]
fn p1_missing_or_invalid_counts_cannot_pass_tool_or_gate() {
    let f = Fixture::new();
    std::fs::create_dir(f.root.join("tests")).unwrap();
    let spec = FixedTest {
        recipe: "python_unittest".into(),
        ..f.test()
    };
    let p = prepare_scoped(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into()],
        vec![spec],
        false,
        None,
    )
    .unwrap();
    approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
    let mut api = FakeApi {
        root: f.root.clone(),
        ..Default::default()
    };
    let mut fake = FakeTests {
        state: "PASS",
        calls: 0,
    };
    let r = task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut api,
        &mut fake,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(r["phase"], "NEEDS_REMEDIATION");
    assert!(task_loop::user_decision_with_vault(&f.db, &f.vault, &p.id, true, "no proof").is_err());
    for raw in [
        r#"{"schema":"nova.unittest/1","discovered":0,"executed":0,"skipped":0,"failures":0,"errors":0,"expected_failures":0,"unexpected_successes":0}"#,
        r#"{"schema":"nova.unittest/1","discovered":2,"executed":2,"skipped":2,"failures":0,"errors":0,"expected_failures":0,"unexpected_successes":0}"#,
    ] {
        let c = recipes::counts(&format!("NOVA_UNIT_RESULT {raw}")).unwrap();
        assert!(!c.passed());
    }
}
#[test]
fn p1_gate_rechecks_counts_and_legacy_plan_requires_new_approval() {
    let f = Fixture::new();
    std::fs::create_dir(f.root.join("tests")).unwrap();
    std::fs::write(
        f.root.join("tests/test_gate.py"),
        "import unittest\nclass T(unittest.TestCase):\n def test_a(self): pass\n",
    )
    .unwrap();
    let spec = FixedTest {
        recipe: "python_unittest".into(),
        ..f.test()
    };
    let p = prepare_scoped(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into()],
        vec![spec],
        false,
        None,
    )
    .unwrap();
    let mut legacy = p.clone();
    legacy.approval_kind.clear();
    assert_eq!(
        validate_current(&f.db, &legacy).err(),
        Some("API_LEGACY_PLAN_REQUIRES_NEW_APPROVAL")
    );
    approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
    let mut a = FakeApi {
        root: f.root.clone(),
        ..Default::default()
    };
    let r = task_loop::execute_loop(
        &f.db,
        &p.id,
        &f.vault,
        &mut a,
        &mut FixedTestRunner,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(r["phase"], "AWAITING_USER");
    let (_, _, _, mut summary) = load_plan(&f.db, &p.id).unwrap();
    summary["tests"][0]["counts"] = Value::Null;
    f.db.conn()
        .execute(
            "UPDATE api_task_runs SET summary_json=?1 WHERE id=?2",
            rusqlite::params![encode(&summary).unwrap(), p.id],
        )
        .unwrap();
    assert!(f
        .db
        .evaluate_acceptance_submission(&f.db.task(&f.task).unwrap())
        .unwrap()
        .is_err());
    assert!(task_loop::user_decision_with_vault(
        &f.db,
        &f.vault,
        &p.id,
        true,
        "reject missing counts"
    )
    .is_err());
    assert!(f.db.user_acceptance_for_task(&f.task).unwrap().is_none());
}
#[test]
fn legacy_grant_public_paths_reject_before_any_reservation() {
    for node in [false, true] {
        for awaiting in [false, true] {
            let source = Fixture::new();
            let mut p = source.plan();
            let f = Fixture::new();
            p.id = uuid::Uuid::new_v4().to_string();
            p.task_id = f.task.clone();
            p.task = task_snapshot(&f.db, &f.task).unwrap();
            p.task_digest = hash(encode(&p.task).unwrap().as_bytes());
            p.project_id = f.db.task(&f.task).unwrap().project_id;
            p.root = f.root.clone();
            p.root_identity = Guard::open(&f.root, true).unwrap().identity;
            p.config = config::load_config(&f.db, &f.config.id).unwrap().view;
            p.allow_repair = true;
            p.files = vec![FileSnapshot {
                path: "note.txt".into(),
                identity: read_file(&f.root.join("note.txt"), 8192).unwrap().1,
                before: std::fs::read_to_string(f.root.join("note.txt")).unwrap(),
                sha256: hash(&std::fs::read(f.root.join("note.txt")).unwrap()),
            }];
            if node {
                let dir = f.root.join("apps/desktop/node_modules/vitest");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("vitest.mjs"), "throw Error('must never run')").unwrap();
                let program = std::env::var_os("PATH")
                    .and_then(|v| {
                        std::env::split_paths(&v)
                            .map(|p| p.join("node.exe"))
                            .find(|p| p.is_file())
                    })
                    .unwrap();
                p.tests = vec![tool_snapshot(
                    &FixedTest {
                        recipe: "nova_vitest_api2".into(),
                        program,
                        ..f.test()
                    },
                    &f.root,
                    &p.files,
                )
                .unwrap()];
            } else {
                p.tests = vec![tool_snapshot(&f.test(), &f.root, &p.files).unwrap()];
            }
            let mut old = serde_json::to_value(&p).unwrap();
            old.as_object_mut().unwrap().remove("approval_kind");
            let raw = encode(&old).unwrap();
            let digest = hash(raw.as_bytes());
            let proposal = json!({"schema":"nova.relay.proposal/1","target_path":"note.txt","old_text":"Alpha","new_text":"beta","rationale":"synthetic prior execution"});
            let candidate = candidate_text(
                "note.txt",
                &p.files[0].sha256,
                p.files[0].before.as_bytes(),
                &serde_json::to_vec(&proposal).unwrap(),
                false,
            )
            .unwrap();
            std::fs::write(f.root.join("note.txt"), candidate.text.as_bytes()).unwrap();
            let summary=encode(&json!({"approved_digest":digest,"changes":[{"path":"note.txt","candidate":candidate}],"tests":[{"file":"note.txt","status":"PASS","exit_code":0,"stdout":"synthetic","stderr":"","counts":null}],"review":{"verdict":"PASS"},"write_verified":true,"published_generation":1})).unwrap();
            f.db.conn()
                .execute(
                    "INSERT INTO api_task_runs VALUES(?1,?2,?3,?4,?5,?6,4,?7)",
                    rusqlite::params![
                        p.id,
                        p.task_id,
                        p.project_id,
                        raw,
                        digest,
                        if awaiting {
                            "AWAITING_USER"
                        } else {
                            "USER_REJECTED"
                        },
                        summary
                    ],
                )
                .unwrap();
            f.db.conn()
                .execute(
                    "INSERT INTO api_task_scopes VALUES(?1,?2,NULL)",
                    rusqlite::params![p.task_id, p.id],
                )
                .unwrap();
            // Build the old post-result Task through existing legal transitions, not an admission override.
            for state in [
                TaskState::Queued,
                TaskState::Running,
                TaskState::Verifying,
                TaskState::PreviewReady,
                TaskState::AwaitingUserAcceptance,
            ] {
                f.db.transition_task(&f.task, state, Actor::Nova, None, None, false, None)
                    .unwrap();
            }
            if awaiting {
                task_loop::user_decision_with_vault(&f.db, &f.vault, &p.id, false, "decline only")
                    .unwrap();
            } else {
                f.db.transition_task(
                    &f.task,
                    TaskState::NeedsUserAction,
                    Actor::User,
                    None,
                    None,
                    false,
                    None,
                )
                .unwrap();
            }
            let before = load_plan(&f.db, &p.id).unwrap();
            let row_before: (String, String, String, i64, String) =
                f.db.conn()
                    .query_row(
                        "SELECT plan_json,digest,phase,revision,summary_json FROM api_task_runs",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                    )
                    .unwrap();
            let bytes = std::fs::read(f.root.join("note.txt")).unwrap();
            let mut api = FakeApi::default();
            let mut tests = FakeTests {
                state: "PASS",
                calls: 0,
            };
            assert_eq!(
                task_loop::execute_loop(
                    &f.db,
                    &p.id,
                    &f.vault,
                    &mut api,
                    &mut tests,
                    &CancellationToken::new()
                )
                .unwrap_err(),
                "API_PARENT_GRANT_REJECTED"
            );
            assert!(api.calls.is_empty());
            assert_eq!(tests.calls, 0);
            assert_eq!(std::fs::read(f.root.join("note.txt")).unwrap(), bytes);
            let after = load_plan(&f.db, &p.id).unwrap();
            assert_eq!(before.1, after.1);
            assert_eq!(before.2, after.2);
            assert_eq!(before.3, after.3);
            let row: (i64, Option<String>) = f
                .db
                .conn()
                .query_row(
                    "SELECT (SELECT count(*) FROM api_task_runs),repair_run FROM api_task_scopes",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(row, (1, None));
            let stored: String =
                f.db.conn()
                    .query_row("SELECT plan_json FROM api_task_runs", [], |r| r.get(0))
                    .unwrap();
            assert_eq!(stored, raw);
            let row_after: (String, String, String, i64, String) =
                f.db.conn()
                    .query_row(
                        "SELECT plan_json,digest,phase,revision,summary_json FROM api_task_runs",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                    )
                    .unwrap();
            assert_eq!(row_before, row_after);
            if awaiting && !node {
                let manual = task_loop::repair(
                    &f.db,
                    &f.vault,
                    &p.id,
                    vec!["note.txt".into()],
                    vec![f.test()],
                    &f.config.id,
                )
                .unwrap();
                assert_eq!(manual.approval_kind, "EXPLICIT");
                assert_eq!(status(&f.db, &manual.id).unwrap()["phase"], "PREPARED");
                assert!(task_loop::execute_loop(
                    &f.db,
                    &manual.id,
                    &f.vault,
                    &mut api,
                    &mut tests,
                    &CancellationToken::new()
                )
                .is_err());
                assert!(api.calls.is_empty());
                assert!(task_loop::repair(
                    &f.db,
                    &f.vault,
                    &p.id,
                    vec!["note.txt".into()],
                    vec![f.test()],
                    &f.config.id
                )
                .is_err());
            }
        }
    }
}
#[test]
fn child_rechecks_persisted_parent_grant_and_link() {
    let f = Fixture::new();
    let p = prepare_scoped(
        &f.db,
        &f.vault,
        &f.task,
        &f.config.id,
        vec!["note.txt".into()],
        vec![f.test()],
        true,
        None,
    )
    .unwrap();
    approve(&f.db, &p.id, &hash(encode(&p).unwrap().as_bytes())).unwrap();
    execute_case(&f, &p, "changes", "PASS");
    let child = task_loop::inherit_repair(&f.db, &f.vault, &p.id, &p).unwrap();
    assert!(validate_current(&f.db, &child).is_ok());
    let mut wrong = child.clone();
    wrong.parent_run = Some(child.id.clone());
    assert!(validate_current(&f.db, &wrong).is_err());
    wrong = child.clone();
    wrong.id = uuid::Uuid::new_v4().to_string();
    assert!(validate_current(&f.db, &wrong).is_err());
    let (_, _, _, mut s) = load_plan(&f.db, &p.id).unwrap();
    s["approved_digest"] = json!("wrong");
    f.db.conn()
        .execute(
            "UPDATE api_task_runs SET summary_json=?1 WHERE id=?2",
            rusqlite::params![encode(&s).unwrap(), p.id],
        )
        .unwrap();
    assert!(validate_current(&f.db, &child).is_err());
    let mut api = FakeApi::default();
    let mut tests = FakeTests {
        state: "PASS",
        calls: 0,
    };
    assert!(task_loop::execute_loop(
        &f.db,
        &child.id,
        &f.vault,
        &mut api,
        &mut tests,
        &CancellationToken::new()
    )
    .is_err());
    assert!(api.calls.is_empty());
    assert_eq!(tests.calls, 0);
}

#[cfg(test)]
mod pm_cases {
    use super::*;
    use crate::api_only::pm;
    struct PmApi {
        calls: Vec<Value>,
        mode: &'static str,
    }
    impl ApiPort for PmApi {
        fn send(
            &mut self,
            r: &transport::WireRequest,
            _: &SecretLease,
            cancel: &CancellationToken,
        ) -> Result<Vec<u8>> {
            let body: Value = decode(&r.body, 131072)?;
            let input: Value = decode(
                body["messages"][1]["content"].as_str().unwrap().as_bytes(),
                131072,
            )?;
            self.calls.push(body.clone());
            assert_eq!(body["messages"].as_array().unwrap().len(), 2);
            assert!(body.get("tools").is_none());
            if self.mode == "unknown" {
                return Err("API_RESPONSE_UNKNOWN");
            }
            if self.mode == "cancel" {
                cancel.cancel();
                return Err("API_CANCELLED_UNKNOWN");
            }
            if self.mode == "expired" {
                TEST_CLOCK.with(|v| v.set(Some(now_utc() + 4000)));
            }
            let output = if input.get("goal").is_some() {
                json!({"schema":"nova.api.pm.draft/1","action":"ARRANGE_DEVELOP","goal":"Change Alpha to beta","requirements":"Change the approved word only","acceptance":"Expected exact beta text; fixed tests and independent review pass","files":if self.mode=="outside"{json!(["../secret"])}else{json!(["note.txt"])},"test_recipes":input["fixed_tests"].as_array().unwrap().iter().map(|v|v["specification"]["recipe"].clone()).collect::<Vec<_>>(),"reason":"Concrete bounded task"})
            } else {
                let allowed = input["allowed_actions"].as_array().unwrap();
                let action = if self.mode == "invalid" {
                    "COMPLETE_TASK"
                } else if self.mode == "skip" {
                    "DELIVER"
                } else if self.mode == "second_repair" && self.calls.len() == 5 {
                    "REPAIR"
                } else {
                    allowed[0].as_str().unwrap()
                };
                json!({"schema":"nova.api.pm.decision/1","action":action,"reason":"Based only on actual tool receipts"})
            };
            let output = if self.mode == "large_invalid" {
                json!({"schema":"invalid","reason":"x".repeat(65400)})
            } else { output };
            Ok(encode(&json!({"model":body["model"],"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":encode(&output)?}}]}))?.into_bytes())
        }
    }
    fn request(f: &Fixture, test: FixedTest) -> pm::Request {
        pm::Request {
            project_id: f.db.task(&f.task).unwrap().project_id,
            goal: "Change Alpha to beta".into(),
            pm_config_id: f.config.id.clone(),
            execution_config_id: f.config.id.clone(),
            paths: vec!["note.txt".into()],
            tests: vec![test],
            allow_repair: true,
        }
    }
    fn prepared(f: &Fixture, test: FixedTest, pm_api: &mut PmApi) -> Value {
        let s = pm::prepare(&f.db, &f.vault, request(f, test)).unwrap();
        pm::plan(
            &f.db,
            &f.vault,
            s["id"].as_str().unwrap(),
            s["digest"].as_str().unwrap(),
            true,
            pm_api,
            &CancellationToken::new(),
        )
        .unwrap()
    }
    fn ready(f: &Fixture, test: FixedTest, api: &mut PmApi) -> Value {
        let s = prepared(f, test, api);
        let s = pm::confirm(&f.db, &f.vault, s["id"].as_str().unwrap()).unwrap();
        pm::approve(
            &f.db,
            s["id"].as_str().unwrap(),
            s["execution_approval_digest"].as_str().unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn pm_transport_facts_legacy_output_and_unknown_do_not_replay() {
        let f=Fixture::new();
        let mut pm_api=DiagnosedPort {inner:PmApi {calls:vec![],mode:""},timeout:0,calls:0};
        let s=pm::prepare(&f.db,&f.vault,request(&f,f.test())).unwrap();
        let id=s["id"].as_str().unwrap();
        let planned=pm::plan(&f.db,&f.vault,id,s["digest"].as_str().unwrap(),true,&mut pm_api,&CancellationToken::new()).unwrap();
        assert_eq!(planned["decisions"][0]["output"]["action"],"ARRANGE_DEVELOP");
        assert_eq!(planned["decisions"][0]["transport"]["body_complete"],true);
        // A genuine old output_json shape has no tool envelope or transport fields.
        let model=encode(&planned["decisions"][0]["output"]).unwrap();
        f.db.conn().execute("UPDATE api_pm_decisions SET output_json=?1 WHERE packet_id=?2",rusqlite::params![model,id]).unwrap();
        let legacy=pm::status(&f.db,id).unwrap();
        assert_eq!(legacy["decisions"][0]["output"]["action"],"ARRANGE_DEVELOP");
        assert!(legacy["decisions"][0]["transport"].is_null());
        let ready=pm::confirm(&f.db,&f.vault,id).unwrap();
        pm::approve(&f.db,id,ready["execution_approval_digest"].as_str().unwrap()).unwrap();
        let mut api=DiagnosedPort {inner:FakeApi::default(),timeout:1,calls:0};
        let mut tests=FakeTests {state:"PASS",calls:0};
        assert_eq!(pm::execute(&f.db,&f.vault,id,&mut pm_api,&mut api,&mut tests,&CancellationToken::new()).unwrap_err(),"API_PM_EXECUTION_UNKNOWN");
        let stopped=pm::status(&f.db,id).unwrap();
        assert_eq!(stopped["execution"]["summary"]["error_code"],"API_TIMEOUT_UNKNOWN");
        assert_eq!(stopped["overall_requests_reserved"],2);
        for _ in 0..2 {assert_eq!(pm::status(&f.db,id).unwrap(),stopped);}
        assert!(pm::execute(&f.db,&f.vault,id,&mut pm_api,&mut api,&mut tests,&CancellationToken::new()).is_err());
        assert_eq!((pm_api.calls,api.calls,tests.calls),(1,1,0));
        let f=Fixture::new();
        let s=pm::prepare(&f.db,&f.vault,request(&f,f.test())).unwrap();
        let mut large=DiagnosedPort {inner:PmApi {calls:vec![],mode:"large_invalid"},timeout:0,calls:0};
        assert!(pm::plan(&f.db,&f.vault,s["id"].as_str().unwrap(),s["digest"].as_str().unwrap(),true,&mut large,&CancellationToken::new()).is_err());
        let stopped=pm::status(&f.db,s["id"].as_str().unwrap()).unwrap();
        assert_eq!(stopped["phase"],"STOPPED");
        assert_eq!(stopped["decisions"][0]["transport"]["body_complete"],true);
        assert!(stopped["decisions"][0]["output"].is_null());
        assert_eq!(large.calls,1);
    }
    #[test]
    fn pm_planning_consent_and_refresh_are_read_only() {
        let f = Fixture::new();
        let s = pm::prepare(&f.db, &f.vault, request(&f, f.test())).unwrap();
        let id = s["id"].as_str().unwrap();
        let mut api = PmApi {
            calls: vec![],
            mode: "",
        };
        assert_eq!(
            pm::plan(
                &f.db,
                &f.vault,
                id,
                s["digest"].as_str().unwrap(),
                false,
                &mut api,
                &CancellationToken::new()
            )
            .unwrap_err(),
            "API_PM_PLANNING_CONSENT_REQUIRED"
        );
        for _ in 0..3 {
            assert_eq!(pm::status(&f.db, id).unwrap()["pm_requests_reserved"], 0);
        }
        assert!(api.calls.is_empty());
        assert_eq!(
            std::fs::read(f.root.join("note.txt")).unwrap(),
            b"Alpha \xc3\x84\r\nend\r\n"
        );
    }
    #[test]
    fn pm_end_to_end_real_unittest_and_user_only_acceptance() {
        let f = Fixture::new();
        std::fs::create_dir(f.root.join("tests")).unwrap();
        // Python recipe intentionally retains -I/-S; test accesses an explicit local fixture path.
        let path = encode(&f.root.join("note.txt").to_string_lossy()).unwrap();
        std::fs::write(f.root.join("tests/test_word.py"),format!("import unittest\nfrom pathlib import Path\nclass Word(unittest.TestCase):\n def test_word(self):\n  self.assertEqual(Path({path}).read_bytes(), b'beta \\xc3\\x84\\r\\nend\\r\\n')\n")).unwrap();
        let mut spec = f.test();
        spec.recipe = "python_unittest".into();
        let mut pm_api = PmApi {
            calls: vec![],
            mode: "",
        };
        let s = ready(&f, spec, &mut pm_api);
        let id = s["id"].as_str().unwrap();
        let mut api = FakeApi {
            root: f.root.clone(),
            ..Default::default()
        };
        let result = pm::execute(
            &f.db,
            &f.vault,
            id,
            &mut pm_api,
            &mut api,
            &mut FixedTestRunner,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(result["phase"], "AWAITING_USER");
        assert_eq!(pm_api.calls.len(), 3);
        assert_eq!(api.calls.len(), 2);
        assert_eq!(
            result["execution"]["summary"]["tests"][0]["counts"]["executed"],
            1
        );
        let task = result["task_id"].as_str().unwrap();
        assert_eq!(
            f.db.task(task).unwrap().state,
            TaskState::AwaitingUserAcceptance
        );
        assert!(pm::execute(
            &f.db,
            &f.vault,
            id,
            &mut pm_api,
            &mut api,
            &mut FixedTestRunner,
            &CancellationToken::new()
        )
        .is_err());
        assert_eq!(api.calls.len(), 2);
        task_loop::user_decision(
            &f.db,
            result["run_id"].as_str().unwrap(),
            true,
            "User accepts",
        )
        .unwrap();
        assert_eq!(f.db.task(task).unwrap().state, TaskState::Completed);
    }
    #[test]
    fn pm_authorized_once_repair_preserves_shared_scope() {
        let f = Fixture::new();
        std::fs::create_dir(f.root.join("tests")).unwrap();
        let path = encode(&f.root.join("note.txt").to_string_lossy()).unwrap();
        std::fs::write(f.root.join("tests/test_repair.py"),format!("import unittest\nfrom pathlib import Path\nclass Word(unittest.TestCase):\n def test_repair(self):\n  self.assertTrue(Path({path}).read_bytes().startswith(b'gamma'))\n")).unwrap();
        let mut spec = f.test();
        spec.recipe = "python_unittest".into();
        let mut pm_api = PmApi {
            calls: vec![],
            mode: "",
        };
        let s = ready(&f, spec, &mut pm_api);
        let mut api = LoopApi {
            calls: 0,
            reviews: 0,
            fail_second: false,
        };
        let r = pm::execute(
            &f.db,
            &f.vault,
            s["id"].as_str().unwrap(),
            &mut pm_api,
            &mut api,
            &mut FixedTestRunner,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(r["phase"], "AWAITING_USER");
        assert_eq!(r["pm_requests_reserved"], 5);
        assert_eq!(api.calls, 4);
        assert_eq!(r["report"]["repair_count"], 1);
        assert_eq!(
            r["report"]["runs"][0]["summary"]["tests"][0]["status"],
            "FAIL"
        );
        assert_eq!(
            r["execution"]["summary"]["tests"][0]["counts"]["executed"],
            1
        );
    }
    #[test]
    fn pm_invalid_skip_review_and_second_repair_are_rejected() {
        for mode in ["invalid", "skip", "second_repair"] {
            let f = Fixture::new();
            let mut pm_api = PmApi {
                calls: vec![],
                mode: "",
            };
            let s = ready(&f, f.test(), &mut pm_api);
            pm_api.mode = mode;
            let mut api = LoopApi {
                calls: 0,
                reviews: 0,
                fail_second: false,
            };
            let mut tests = FakeTests {
                state: "PASS",
                calls: 0,
            };
            assert!(pm::execute(
                &f.db,
                &f.vault,
                s["id"].as_str().unwrap(),
                &mut pm_api,
                &mut api,
                &mut tests,
                &CancellationToken::new()
            )
            .is_err());
            assert_ne!(
                f.db.task(s["task_id"].as_str().unwrap()).unwrap().state,
                TaskState::Completed
            );
            if mode != "second_repair" {
                assert_eq!(api.calls, 1);
            } else {
                assert_eq!(api.calls, 4);
            }
        }
    }
    #[test]
    fn pm_failed_tests_cannot_be_delivered() {
        let f = Fixture::new();
        let mut pm_api = PmApi {
            calls: vec![],
            mode: "",
        };
        let s = ready(&f, f.test(), &mut pm_api);
        // Review still happens, then an impossible DELIVER action must be rejected.
        struct SkipAfter(PmApi);
        impl ApiPort for SkipAfter {
            fn send(
                &mut self,
                r: &transport::WireRequest,
                k: &SecretLease,
                c: &CancellationToken,
            ) -> Result<Vec<u8>> {
                if self.0.calls.len() == 2 {
                    self.0.mode = "skip"
                }
                self.0.send(r, k, c)
            }
        }
        let mut pm_api = SkipAfter(pm_api);
        let mut api = FakeApi {
            root: f.root.clone(),
            ..Default::default()
        };
        let mut tests = FakeTests {
            state: "FAIL",
            calls: 0,
        };
        assert!(pm::execute(
            &f.db,
            &f.vault,
            s["id"].as_str().unwrap(),
            &mut pm_api,
            &mut api,
            &mut tests,
            &CancellationToken::new()
        )
        .is_err());
        assert_eq!(api.calls.len(), 2);
        assert_eq!(tests.calls, 1);
        assert_eq!(
            f.db.task(s["task_id"].as_str().unwrap()).unwrap().state,
            TaskState::Confirmed
        );
    }
    #[test]
    fn pm_scope_invalid_response_failure_cancel_unknown_no_replay() {
        for mode in ["outside", "unknown", "cancel"] {
            let f = Fixture::new();
            let s = pm::prepare(&f.db, &f.vault, request(&f, f.test())).unwrap();
            let id = s["id"].as_str().unwrap();
            let mut api = PmApi {
                calls: vec![],
                mode,
            };
            assert!(pm::plan(
                &f.db,
                &f.vault,
                id,
                s["digest"].as_str().unwrap(),
                true,
                &mut api,
                &CancellationToken::new()
            )
            .is_err());
            assert!(pm::plan(
                &f.db,
                &f.vault,
                id,
                s["digest"].as_str().unwrap(),
                true,
                &mut api,
                &CancellationToken::new()
            )
            .is_err());
            assert_eq!(api.calls.len(), 1);
            assert!(pm::status(&f.db, id).unwrap()["task_id"].is_null());
            assert_eq!(
                std::fs::read(f.root.join("note.txt")).unwrap(),
                b"Alpha \xc3\x84\r\nend\r\n"
            );
        }
    }
    #[test]
    fn pm_config_drift_and_expiry_stop_before_send() {
        for expired in [false, true] {
            let f = Fixture::new();
            let mut api = PmApi {
                calls: vec![],
                mode: "",
            };
            let s = ready(&f, f.test(), &mut api);
            if expired {
                TEST_CLOCK.with(|v| v.set(Some(now_utc() + 4000)));
            } else {
                let mut cfg = f.config.clone();
                cfg.model = "changed-selected-model".into();
                config::save_config(&f.db, &f.vault, cfg, None).unwrap();
            }
            let mut dev = FakeApi::default();
            let mut tests = FakeTests {
                state: "PASS",
                calls: 0,
            };
            assert!(pm::execute(
                &f.db,
                &f.vault,
                s["id"].as_str().unwrap(),
                &mut api,
                &mut dev,
                &mut tests,
                &CancellationToken::new()
            )
            .is_err());
            assert_eq!(api.calls.len(), 1);
            assert!(dev.calls.is_empty());
            assert_eq!(tests.calls, 0);
            TEST_CLOCK.with(|v| v.set(None));
        }
    }
    #[test]
    fn pm_response_crossing_deadline_does_not_create_task() {
        let f = Fixture::new();
        let s = pm::prepare(&f.db, &f.vault, request(&f, f.test())).unwrap();
        let mut api = PmApi {
            calls: vec![],
            mode: "expired",
        };
        let r = pm::plan(
            &f.db,
            &f.vault,
            s["id"].as_str().unwrap(),
            s["digest"].as_str().unwrap(),
            true,
            &mut api,
            &CancellationToken::new(),
        );
        TEST_CLOCK.with(|v| v.set(None));
        assert_eq!(r.unwrap_err(), "API_APPROVAL_EXPIRED");
        assert!(pm::status(&f.db, s["id"].as_str().unwrap()).unwrap()["task_id"].is_null());
    }
    #[test]
    fn pm_different_configuration_references_are_preserved() {
        let f = Fixture::new();
        let mut pmconfig = f.config.clone();
        pmconfig.id = uuid::Uuid::new_v4().to_string();
        pmconfig.model = "independent-pm-model".into();
        config::save_config(
            &f.db,
            &f.vault,
            pmconfig.clone(),
            Some("SYNTHETIC_PM_SECRET_123456789".into()),
        )
        .unwrap();
        let mut req = request(&f, f.test());
        req.pm_config_id = pmconfig.id.clone();
        let s = pm::prepare(&f.db, &f.vault, req).unwrap();
        let mut api = PmApi {
            calls: vec![],
            mode: "",
        };
        let s = pm::plan(
            &f.db,
            &f.vault,
            s["id"].as_str().unwrap(),
            s["digest"].as_str().unwrap(),
            true,
            &mut api,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(api.calls[0]["model"], pmconfig.model);
        assert_eq!(s["packet"]["execution"]["config"]["id"], f.config.id);
    }
    #[test]
    fn pm_direct_api_loop_cannot_bypass_coordination() {
        let f = Fixture::new();
        let mut pm_api = PmApi {
            calls: vec![],
            mode: "",
        };
        let s = ready(&f, f.test(), &mut pm_api);
        let mut api = FakeApi {
            root: f.root.clone(),
            ..Default::default()
        };
        let mut tests = FakeTests {
            state: "PASS",
            calls: 0,
        };
        assert_eq!(
            task_loop::execute_loop(
                &f.db,
                s["run_id"].as_str().unwrap(),
                &f.vault,
                &mut api,
                &mut tests,
                &CancellationToken::new()
            )
            .unwrap_err(),
            "API_PM_COORDINATOR_REQUIRED"
        );
        assert!(api.calls.is_empty());
    }
    #[test]
    fn pm_mid_handoff_failure_unknown_cancel_and_reopen_do_not_send_review() {
        for mode in ["unknown", "cancel", "invalid"] {
            let f = Fixture::new();
            let mut pm_api = PmApi {
                calls: vec![],
                mode: "",
            };
            let s = ready(&f, f.test(), &mut pm_api);
            pm_api.mode = mode;
            let mut dev = FakeApi {
                root: f.root.clone(),
                ..Default::default()
            };
            let mut tests = FakeTests {
                state: "PASS",
                calls: 0,
            };
            let id = s["id"].as_str().unwrap();
            assert!(pm::execute(
                &f.db,
                &f.vault,
                id,
                &mut pm_api,
                &mut dev,
                &mut tests,
                &CancellationToken::new()
            )
            .is_err());
            assert_eq!(dev.calls.len(), 1);
            assert_eq!(tests.calls, 1);
            assert_eq!(pm_api.calls.len(), 2);
            let saved = pm::status(&f.db, id).unwrap();
            if mode != "invalid" {
                assert_eq!(saved["phase"], "UNKNOWN")
            }
            let path = f.root.join("reopened.sqlite3");
            f.db.conn()
                .execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])
                .unwrap();
            let db = Database::open(&path).unwrap();
            assert!(pm::execute(
                &db,
                &f.vault,
                id,
                &mut pm_api,
                &mut dev,
                &mut tests,
                &CancellationToken::new()
            )
            .is_err());
            assert_eq!(dev.calls.len(), 1);
            assert_eq!(pm_api.calls.len(), 2);
            assert_eq!(pm::status(&db, id).unwrap()["revision"], saved["revision"]);
        }
    }
    #[test]
    fn pm_started_planning_and_running_reservations_survive_reopen() {
        for starting in ["PLANNING_STARTED", "RUNNING"] {
            let f = Fixture::new();
            let mut api = PmApi {
                calls: vec![],
                mode: "",
            };
            let s = if starting == "RUNNING" {
                ready(&f, f.test(), &mut api)
            } else {
                pm::prepare(&f.db, &f.vault, request(&f, f.test())).unwrap()
            };
            let id = s["id"].as_str().unwrap();
            f.db.conn()
                .execute(
                    "UPDATE api_pm_packets SET phase=?1 WHERE id=?2",
                    rusqlite::params![starting, id],
                )
                .unwrap();
            let path = f.root.join("started.sqlite3");
            f.db.conn()
                .execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])
                .unwrap();
            let db = Database::open(&path).unwrap();
            let count = api.calls.len();
            assert!(pm::plan(
                &db,
                &f.vault,
                id,
                s["digest"].as_str().unwrap(),
                true,
                &mut api,
                &CancellationToken::new()
            )
            .is_err());
            let mut dev = FakeApi::default();
            let mut tests = FakeTests {
                state: "PASS",
                calls: 0,
            };
            assert!(pm::execute(
                &db,
                &f.vault,
                id,
                &mut api,
                &mut dev,
                &mut tests,
                &CancellationToken::new()
            )
            .is_err());
            assert_eq!(api.calls.len(), count);
            assert!(dev.calls.is_empty());
        }
    }
    #[test]
    fn pm_material_secrets_and_file_drift_are_rejected_without_api() {
        let f = Fixture::new();
        let mut req = request(&f, f.test());
        req.goal = "SYNTHETIC_SECRET_VALUE_12345678".into();
        assert!(pm::prepare(&f.db, &f.vault, req).is_err());
        let mut api = PmApi {
            calls: vec![],
            mode: "",
        };
        let s = ready(&f, f.test(), &mut api);
        std::fs::write(f.root.join("note.txt"), b"user edit").unwrap();
        let mut dev = FakeApi::default();
        let mut tests = FakeTests {
            state: "PASS",
            calls: 0,
        };
        assert_eq!(
            pm::execute(
                &f.db,
                &f.vault,
                s["id"].as_str().unwrap(),
                &mut api,
                &mut dev,
                &mut tests,
                &CancellationToken::new()
            )
            .unwrap_err(),
            "API_FILE_DRIFT"
        );
        assert!(dev.calls.is_empty());
        let snapshot = encode(&pm::status(&f.db, s["id"].as_str().unwrap()).unwrap()).unwrap();
        assert!(!snapshot.contains("SYNTHETIC_SECRET_VALUE_12345678"));
    }
}
