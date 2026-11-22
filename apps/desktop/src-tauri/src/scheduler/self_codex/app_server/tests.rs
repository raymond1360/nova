//! Actual local fake process through the same Windows Tree/Job. No CLI/model or permission grant.
use super::*;
use std::io::{BufRead, Write};
thread_local! {static TEST_ROOT:std::cell::RefCell<Option<PathBuf>>=const {std::cell::RefCell::new(None)};}
pub(crate) fn test_cli_binding(p: &Plan) -> bool {
    TEST_ROOT.with(|v| v.borrow().as_ref() == Some(&p.project_root))
        && matches!(p.mode, Mode::Synthetic)
        && std::env::current_exe().is_ok_and(|exe| {
            exe == p.cli && tool_digest(&exe).ok().as_deref() == Some(p.cli_sha256.as_str())
        })
}
struct Scope;
impl Drop for Scope {
    fn drop(&mut self) {
        TEST_ROOT.with(|v| v.replace(None));
    }
}
fn emit(v: Value) {
    // Synthetic producer evidence only, never a logger for real configuration/model frames.
    let path = std::env::current_dir()
        .unwrap()
        .parent()
        .unwrap()
        .join("fake-frames.jsonl");
    let mut trace = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(trace, "{v}").unwrap();
    println!("{v}");
    std::io::stdout().flush().unwrap()
}
fn result(id: &Value, v: Value) {
    emit(json!({"id":id,"result":v}));
}
fn event(method: &str, p: Value) {
    emit(json!({"method":method,"params":p}));
}
pub(crate) fn fake_child(case: &str) -> ! {
    let case = case.strip_prefix("app-server:").unwrap();
    let root = std::env::current_dir().unwrap();
    let dev = uuid::Uuid::new_v4().to_string();
    let review = uuid::Uuid::new_v4().to_string();
    let dev_session = uuid::Uuid::new_v4().to_string();
    let rev_session = uuid::Uuid::new_v4().to_string();
    let mut n = 0;
    let trace = root.parent().unwrap().join("fake-rpc.jsonl");
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&trace)
            .unwrap();
        writeln!(f, "{v}").unwrap();
        f.sync_all().unwrap();
        if case == "disconnect" {
            std::process::exit(7)
        }
        if case == "timeout" || case == "cancel" {
            std::thread::sleep(Duration::from_secs(10));
            std::process::exit(7)
        }
        let p = &v["params"];
        let id = &v["id"];
        match v["method"].as_str().unwrap() {
            "initialize" => result(id, json!({"userAgent":"SYNTHETIC ONLY"})),
            "initialized" => {}
            "config/read" => {
                let mut c = json!({"model":"EXPLICIT_SYNTHETIC_MODEL","model_provider":"openai","mcp_servers":{},"project_doc_max_bytes":0,
                    "features":{"plugins":false,"hooks":false,"plugin_hooks":false,"multi_agent":false,"multi_agent_v2":false}});
                if case == "config" {
                    c["mcp_servers"] = json!({"unapproved":{}})
                }
                result(id, json!({"config":c}));
            }
            "thread/loaded/list" => result(
                id,
                json!({"data":if case=="unloaded" {vec![]}else{vec![dev.clone(),review.clone()]},"nextCursor":null}),
            ),
            "thread/start" | "thread/resume" => {
                let is_review = n > 0;
                let resume = v["method"] == "thread/resume";
                let uid = if is_review {
                    if case == "same-uuid" {
                        &dev
                    } else {
                        &review
                    }
                } else {
                    &dev
                };
                let mut session = if is_review {
                    rev_session.clone()
                } else {
                    dev_session.clone()
                };
                if resume && case == "cold" {
                    session = uuid::Uuid::new_v4().to_string()
                }
                let policy = if is_review {
                    json!({"type":"readOnly","networkAccess":false})
                } else {
                    json!({"type":"workspaceWrite","writableRoots":[],"networkAccess":false,"excludeTmpdirEnvVar":true,"excludeSlashTmp":true})
                };
                let mut r = json!({"thread":{"id":uid,"sessionId":session,"status":{"type":"idle"}},"model":"EXPLICIT_SYNTHETIC_MODEL","modelProvider":"openai",
                    "cwd":root,"runtimeWorkspaceRoots":[],"approvalPolicy":"never","approvalsReviewer":"user","activePermissionProfile":null,"sandbox":policy,"reasoningEffort":"low","serviceTier":"default","disabledPluginIds":[]});
                match case {
                    "permission" => r["sandbox"]["networkAccess"] = json!(true),
                    "uuid" => r["thread"]["id"] = json!("NOT A UUID"),
                    "model" => r["model"] = json!("other"),
                    "approver" => r["approvalsReviewer"] = json!("auto_review"),
                    "profile" => r["activePermissionProfile"] = json!({"id":"unapproved"}),
                    "cwd" => r["cwd"] = json!("C:/elsewhere"),
                    "missing" => {
                        r.as_object_mut().unwrap().remove("sandbox");
                    }
                    "r1-permission" if is_review => r["sandbox"]["type"] = json!("workspaceWrite"),
                    _ => {}
                }
                let mut notice = r["thread"].clone();
                if case == "thread-notify-wrong-thread" {
                    notice["id"] = json!(uuid::Uuid::new_v4().to_string())
                }
                if case == "thread-notify-wrong-session" {
                    notice["sessionId"] = json!(uuid::Uuid::new_v4().to_string())
                }
                if !resume && case == "thread-notify-order" {
                    event("thread/started", json!({"thread":notice}));
                }
                result(id, r.clone());
                if case == "duplicate-response" {
                    result(id, r);
                }
                // Frozen thread_processor.rs:1638-1654: reply, then ThreadStarted notification.
                if !resume && case != "thread-notify-missing" && case != "thread-notify-order" {
                    if case == "thread-notify-delayed" {
                        std::thread::sleep(Duration::from_millis(30));
                    }
                    event("thread/started", json!({"thread":notice}));
                    if case == "thread-notify-duplicate" {
                        event("thread/started", json!({"thread":notice}));
                    }
                }
            }
            "turn/start" => {
                n += 1;
                let tid = format!("synthetic-turn-{n}");
                let thread = p["threadId"].as_str().unwrap();
                result(
                    &if case == "turn-ack-wrong-id" {
                        json!(id.as_u64().unwrap() + 1)
                    } else {
                        id.clone()
                    },
                    json!({"turn":{"id":tid,"status":"inProgress"}}),
                );
                if case == "out-of-order" {
                    event(
                        "turn/completed",
                        json!({"threadId":thread,"turn":{"id":tid,"status":"completed"}}),
                    );
                    continue;
                }
                if case != "turn-notify-missing" {
                    event(
                        "turn/started",
                        json!({"threadId":if case=="turn-notify-wrong-thread" {&review}else{thread},"turn":{"id":if case=="turn-notify-wrong-id" {"wrong-turn"}else{&tid},"status":"inProgress"}}),
                    );
                }
                if case == "notification-bound" {
                    for i in 0..70 {
                        let item = json!({"id":format!("reason-{i}"),"type":"reasoning"});
                        event(
                            "item/started",
                            json!({"threadId":thread,"turnId":tid,"item":item}),
                        );
                        event(
                            "item/completed",
                            json!({"threadId":thread,"turnId":tid,"item":item}),
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                if case == "duplicate-event" {
                    event(
                        "turn/started",
                        json!({"threadId":if case=="turn-notify-wrong-thread" {&review}else{thread},"turn":{"id":if case=="turn-notify-wrong-id" {"wrong-turn"}else{&tid},"status":"inProgress"}}),
                    );
                }
                if case == "unexpected-config" {
                    event("thread/settings/updated", json!({"threadId":thread}));
                }
                if n == 1 {
                    fs::write(root.join("a.txt"), b"SELF2_CANARY_READY\n").unwrap();
                }
                if n == 2 && case == "review-write" {
                    fs::write(root.join("a.txt"), b"FORBIDDEN\n").unwrap();
                }
                if case == "active-item" {
                    event(
                        "item/started",
                        json!({"threadId":thread,"turnId":tid,"item":{"id":"busy","type":"commandExecution"}}),
                    );
                }
                if case == "child" && n == 1 {
                    let _child = std::process::Command::new(std::env::current_exe().unwrap())
                        .args([
                            "--exact",
                            "scheduler::admission_probe::tests::probe_fake_child",
                            "--quiet",
                            "--nocapture",
                        ])
                        .env("NOVA_PERMISSION_FAKE_CASE", "wait")
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .unwrap();
                }
                let wire = fixture_wire(p["input"][0]["text"].as_str().unwrap().as_bytes(), thread)
                    .unwrap();
                let agent: Value =
                    serde_json::from_slice(wire.split(|b| *b == b'\n').nth(2).unwrap()).unwrap();
                let mut text = agent["item"]["text"].clone();
                if case == "result" {
                    text = json!("{}");
                }
                event(
                    "item/started",
                    json!({"threadId":thread,"turnId":tid,"item":{"id":"answer","type":"agentMessage"}}),
                );
                event(
                    "item/completed",
                    json!({"threadId":thread,"turnId":tid,"item":{"id":"answer","type":"agentMessage","text":text}}),
                );
                event(
                    "turn/completed",
                    json!({"threadId":thread,"turn":{"id":tid,"status":"completed"}}),
                );
            }
            _ => std::process::exit(8),
        }
    }
    std::process::exit(if case == "exit-on-close" { 7 } else { 0 })
}
#[cfg(windows)]
pub(crate) fn offline_matrix() {
    use h::tests::F;
    assert_eq!(
        app_server_admission().unwrap_err(),
        "SELF2_APP_SERVER_NATIVE_ADMISSION_CLOSED"
    );
    let exe = std::env::current_exe().unwrap();
    let exe_hash = tool_digest(&exe).unwrap();
    let mut evidence = vec![];
    for (case, turns) in [
        ("ok", 3),
        ("exit-on-close", 3),
        ("permission", 0),
        ("uuid", 0),
        ("model", 0),
        ("approver", 0),
        ("profile", 0),
        ("cwd", 0),
        ("missing", 0),
        ("config", 0),
        ("duplicate-response", 0),
        ("thread-notify-delayed", 3),
        ("thread-notify-wrong-thread", 0),
        ("thread-notify-wrong-session", 0),
        ("thread-notify-missing", 0),
        ("thread-notify-order", 0),
        ("thread-notify-duplicate", 1),
        ("turn-ack-wrong-id", 1),
        ("turn-notify-missing", 1),
        ("turn-notify-wrong-thread", 1),
        ("turn-notify-wrong-id", 1),
        ("notification-bound", 1),
        ("r1-permission", 1),
        ("same-uuid", 1),
        ("cold", 2),
        ("unloaded", 2),
        ("packet", 1),
        ("partial", 0),
        ("disconnect", 0),
        ("timeout", 0),
        ("cancel", 0),
        ("out-of-order", 1),
        ("duplicate-event", 1),
        ("unexpected-config", 1),
        ("active-item", 1),
        ("child", 1),
        ("review-write", 2),
        ("result", 1),
    ] {
        let f = F::new();
        TEST_ROOT.with(|v| v.replace(Some(f.repo.clone())));
        let _scope = Scope;
        let mut stage = f.stage.clone();
        stage.developer = None;
        stage.reviewer = None;
        let mut input = f.input.clone();
        input.logs.clear();
        input.required_commands.clear();
        input.developer_report.claimed_conclusion = Conclusion::NotRun;
        let store = h::init_work(
            &h::store_path(&f.repo).unwrap(),
            &f.repo,
            &f.logs,
            stage,
            input.clone(),
        )
        .unwrap();
        let plan = Plan {
            mode: Mode::Synthetic,
            schema: "nova.self2.plan/1".into(),
            input,
            project_root: f.repo.clone(),
            cli: exe.clone(),
            cli_sha256: exe_hash.clone(),
            cli_version: CLI_VERSION.into(),
            model: "EXPLICIT_SYNTHETIC_MODEL".into(),
            commands: vec![],
        };
        attach(&store, plan).unwrap();
        let tmp = f.dir.join("runtime");
        fs::create_dir(&tmp).unwrap();
        let parent = cli_adapter::ENV_ALLOW
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect();
        let mut env = cli_adapter::environment(&parent, &tmp).unwrap();
        env.insert(
            "NOVA_PERMISSION_FAKE_CASE".into(),
            format!("app-server:{case}"),
        );
        let spec = Spec {
            role: RelayRole::Develop,
            profile: "SELF2_APP_SERVER".into(),
            exe: exe.clone(),
            exe_sha256: exe_hash.clone(),
            argv: vec![
                "--exact".into(),
                "scheduler::admission_probe::tests::probe_fake_child".into(),
                "--quiet".into(),
                "--nocapture".into(),
            ],
            env,
            cwd: f.repo.clone(),
            session: String::new(),
        };
        let binding = TransportBinding {
            spec_sha256: spec_hash(&spec).unwrap(),
            plan_sha256: journal(&store).unwrap().plan_sha256,
            root: f.repo.clone(),
            immutable: canary_tree(&f.repo).unwrap(),
            before: fs::read(f.repo.join("a.txt")).unwrap(),
            after: b"SELF2_CANARY_READY\n".to_vec(),
            case: case.into(),
        };
        let out = execute(&store, &binding, spec.clone());
        let calls: Vec<Value> = fs::read_to_string(f.dir.join("fake-rpc.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            calls.iter().filter(|v| v["method"] == "turn/start").count(),
            turns,
            "{case}: {out:?}"
        );
        let app = app_state(&store).unwrap();
        assert_eq!(
            app["process_exit"]["reclaim"], "CONFIRMED",
            "{case}: {out:?}"
        );
        assert_eq!(app["cleanup"]["job_confirmed"], true);
        assert!(app["pid"].as_u64().unwrap() > 0);
        if case == "unloaded" {
            assert!(!calls.iter().any(|v| v["method"] == "thread/resume"));
        }
        if ["ok", "thread-notify-delayed"].contains(&case) {
            let summary = out.as_ref().unwrap_or_else(|e| panic!("{case}: {e}"));
            assert_eq!(summary["self1"]["active"]["status"], "AWAITING_PM_AND_USER");
            assert_eq!(journal(&store).unwrap().records.len(), 3);
            assert!(journal(&store)
                .unwrap()
                .records
                .iter()
                .all(|r| r.attempt.state == "TURN_COMPLETE" && r.attempt.exit_code.is_none()));
            assert_ne!(
                calls
                    .iter()
                    .filter(|v| v["method"] == "turn/start")
                    .nth(0)
                    .unwrap()["params"]["threadId"],
                calls
                    .iter()
                    .filter(|v| v["method"] == "turn/start")
                    .nth(1)
                    .unwrap()["params"]["threadId"]
            );
            let resume = calls
                .iter()
                .find(|v| v["method"] == "thread/resume")
                .unwrap();
            assert_eq!(
                resume["params"]["threadId"],
                calls
                    .iter()
                    .filter(|v| v["method"] == "turn/start")
                    .nth(2)
                    .unwrap()["params"]["threadId"]
            );
        } else {
            assert!(out.is_err(), "{case}");
        }
        let terminal = journal(&store)
            .unwrap()
            .records
            .last()
            .and_then(|r| r.attempt.terminal.clone());
        let expected = match case {
            "thread-notify-wrong-thread" | "thread-notify-wrong-session" => {
                Some("APP_SERVER_NOTIFICATION_BINDING_REJECTED")
            }
            "thread-notify-missing" => Some("DEADLINE"),
            "thread-notify-order" | "duplicate-response" => {
                Some("APP_SERVER_NOTIFICATION_ORDER_REJECTED")
            }
            "thread-notify-duplicate" => Some("APP_SERVER_NOTIFICATION_DUPLICATE"),
            "turn-notify-wrong-thread" => Some("APP_SERVER_EVENT_BINDING_REJECTED"),
            "turn-notify-wrong-id" | "turn-notify-missing" => {
                Some("APP_SERVER_EVENT_ORDER_REJECTED")
            }
            "turn-ack-wrong-id" => Some("APP_SERVER_RPC_ORDER_REJECTED"),
            "notification-bound" => Some("APP_SERVER_FRAME_BOUND"),
            _ => None,
        };
        if let Some(expected) = expected {
            assert_eq!(terminal.as_deref(), Some(expected), "{case}");
        }
        if ["ok", "thread-notify-delayed"].contains(&case) {
            assert_eq!(app["notifications"].as_array().unwrap().len(), 2);
            let frames: Vec<Value> = fs::read_to_string(f.dir.join("fake-frames.jsonl"))
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            for request in calls.iter().filter(|v| v["method"] == "thread/start") {
                let pos = frames
                    .iter()
                    .position(|v| v["id"] == request["id"])
                    .unwrap();
                assert_eq!(frames[pos + 1]["method"], "thread/started");
                assert_eq!(
                    frames[pos + 1]["params"]["thread"],
                    frames[pos]["result"]["thread"]
                );
            }
        }
        let before = bytes(&h::load(&store).unwrap()).unwrap();
        let reopened = Store::self_open(&h::store_path(&f.repo).unwrap()).unwrap();
        assert_eq!(
            execute(&reopened, &binding, spec.clone()).unwrap_err(),
            "APP_SERVER_ALREADY_RESERVED"
        );
        assert_eq!(before, bytes(&h::load(&reopened).unwrap()).unwrap());
        assert_eq!(
            calls.len(),
            fs::read_to_string(f.dir.join("fake-rpc.jsonl"))
                .unwrap_or_default()
                .lines()
                .count()
        );
        let mut wrong = spec;
        wrong.argv.push("arbitrary".into());
        assert_eq!(binding.check(&wrong), Err("APP_SERVER_BINDING_REJECTED"));
        evidence.push(json!({"case":case,"outcome":out.as_ref().map(|_|"SUCCESS").unwrap_or_else(|e|e),"observed_turns":turns,"app":app,"path":f.dir}));
    }
    let dir =
        std::env::temp_dir().join(format!("nova-app-server-evidence-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("matrix.json"),
        bytes(&json!({"synthetic_only":true,"cases":evidence})).unwrap(),
    )
    .unwrap();
    println!("APP_SERVER_OFFLINE_EVIDENCE={}", dir.display());
}
