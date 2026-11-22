use super::*;
struct Scope;
impl Drop for Scope {
    fn drop(&mut self) {
        SOURCE.with(|s| s.replace(None));
    }
}
pub(crate) fn offline_matrix() {
    assert_eq!(
        app_server_p("probe", Value::Null, "").unwrap_err(),
        "P_HOST_TOOL_INITIALIZATION_UNVERIFIED"
    );
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/p-probe-tests");
    fs::create_dir_all(&base).unwrap();
    let mut evidence = vec![];
    for case in [
        "ok",
        "error",
        "disconnect",
        "timeout",
        "cancel",
        "no-approval",
        "expired",
        "wrong-binding",
        "wrong-env",
        "material-drift",
    ] {
        let root = base.join(uuid::Uuid::new_v4().to_string());
        let exe = std::env::current_exe().unwrap();
        SOURCE.with(|s| {
            s.replace(Some(Context {
                root: root.clone(),
                cli: exe.clone(),
                consumer: exe,
                starts: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                case: if [
                    "ok",
                    "no-approval",
                    "expired",
                    "wrong-binding",
                    "wrong-env",
                    "material-drift",
                ]
                .contains(&case)
                {
                    "ok".into()
                } else {
                    case.into()
                },
            }))
        });
        let _scope = Scope;
        let c = Conditions {
            model: "EXPLICIT_SYNTHETIC_MODEL".into(),
            expires_utc: chrono::Utc::now().timestamp() + 3600,
            initialization_terms:
                "SYNTHETIC ONLY; internal startup effects accepted for fake process".into(),
        };
        let prepared = app_server_p("prepare", serde_json::to_value(c).unwrap(), "").unwrap();
        assert_eq!(prepared["status"], "P_DRAFT");
        assert_eq!(
            prepare(Conditions {
                model: "x".into(),
                expires_utc: chrono::Utc::now().timestamp() + 3600,
                initialization_terms: "x".into()
            })
            .unwrap_err(),
            "P_ROOT_EXISTS"
        );
        let s = open().unwrap();
        let digest = app_state(&s).unwrap()["digest"]
            .as_str()
            .unwrap()
            .to_string();
        if case != "no-approval" {
            let before = bytes(&h::load(&s).unwrap()).unwrap();
            assert_eq!(
                app_server_p("approve", json!(digest), "WRONG").unwrap_err(),
                "P_APPROVAL_REJECTED"
            );
            assert_eq!(before, bytes(&h::load(&s).unwrap()).unwrap());
            app_server_p(
                "approve",
                json!(digest),
                &format!("AUTHORIZE P ZERO TURN 1 SERVER {digest}"),
            )
            .unwrap();
        }
        if ["expired", "wrong-binding"].contains(&case) {
            let (rev, mut st) = h::load(&s).unwrap();
            let a = st.self2.as_mut().unwrap().app_server.as_mut().unwrap();
            if case == "expired" {
                let mut r: Request = serde_json::from_value(a["probe_request"].clone()).unwrap();
                r.expires = chrono::Utc::now().timestamp() - 1;
                a["digest"] = json!(hash(&bytes(&r).unwrap()));
                a["probe_request"] = serde_json::to_value(r).unwrap();
            } else {
                a["probe_request"]["consumer_hash"] = json!("0".repeat(64));
            }
            h::save(&s, rev, &st).unwrap();
        }
        if case == "ok" {
            let before = bytes(&h::load(&s).unwrap()).unwrap();
            assert_eq!(
                approve_store(
                    &s,
                    &digest,
                    &format!("AUTHORIZE P ZERO TURN 1 SERVER {digest}")
                )
                .unwrap_err(),
                "P_NO_REPLAY"
            );
            assert_eq!(before, bytes(&h::load(&s).unwrap()).unwrap());
        }
        if case == "wrong-env" {
            let (rev, mut st) = h::load(&s).unwrap();
            st.self2.as_mut().unwrap().app_server.as_mut().unwrap()["probe_request"]["env"]
                [REMOTE] = json!("0");
            h::save(&s, rev, &st).unwrap();
        }
        if case == "material-drift" {
            fs::write(root.join("project/a.txt"), b"DRIFT").unwrap();
        }
        let before = bytes(&h::load(&s).unwrap()).unwrap();
        let signal = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if case == "cancel" {
            let c = signal.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                c.store(true, std::sync::atomic::Ordering::Release);
            });
        }
        let result = run_store(&s, if case == "cancel" { Some(signal) } else { None });
        let trace = root.join("fake-rpc.jsonl");
        let calls: Vec<Value> = fs::read_to_string(&trace)
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert!(calls.len() <= 8);
        assert!(calls.iter().all(|v| v["method"] != "turn/start"));
        let state = app_state(&s).unwrap();
        let opens = SOURCE.with(|s| {
            s.borrow()
                .as_ref()
                .unwrap()
                .starts
                .load(std::sync::atomic::Ordering::SeqCst)
        });
        assert_eq!(
            opens,
            if [
                "no-approval",
                "expired",
                "wrong-binding",
                "wrong-env",
                "material-drift"
            ]
            .contains(&case)
            {
                0
            } else {
                1
            }
        );
        if case == "ok" {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(calls.len(), 8);
            assert_eq!(state["status"], "P_COMPLETE");
            assert_eq!(state["notifications"].as_array().unwrap().len(), 2);
            let mut gate = WireGate::new("EXPLICIT_SYNTHETIC_MODEL", &root.join("project"));
            for (i, call) in calls.iter().enumerate() {
                let mut bad = call.clone();
                bad["params"] = json!({"cwd":"C:/outside","dynamicTools":["UNAUTHORIZED"]});
                let mut wire = bytes(&bad).unwrap();
                wire.push(b'\n');
                assert!(gate.check(&wire).is_err());
                let mut wire = bytes(call).unwrap();
                wire.push(b'\n');
                gate.check(&wire).unwrap();
                gate.advance();
                if i == 4 {
                    gate.bind(
                        state["notifications"][0]["thread"].as_str().unwrap(),
                        state["notifications"][1]["thread"].as_str().unwrap(),
                    )
                    .unwrap();
                }
            }
            assert_eq!(
                gate.check(b"{\"method\":\"initialized\"}\n"),
                Err("P_FRAME_REJECTED")
            );
        } else {
            assert!(result.is_err(), "{case}");
            let expected = match case {
                "no-approval" => "P_NO_APPROVAL_OR_REPLAY",
                "expired" => "P_EXPIRED",
                "wrong-binding" | "wrong-env" | "material-drift" => "P_BINDING_REJECTED",
                "error" => "APP_SERVER_RPC_ORDER_REJECTED",
                "disconnect" => "APP_SERVER_IO_UNKNOWN",
                "timeout" => "DEADLINE",
                "cancel" => "CANCELLED",
                _ => unreachable!(),
            };
            if case == "disconnect" {
                assert!(["APP_SERVER_IO_UNKNOWN", "APP_SERVER_DISCONNECTED"]
                    .contains(&result.as_ref().unwrap_err()));
            } else {
                assert_eq!(result.as_ref().unwrap_err(), &expected, "{case}");
            }
        }
        if [
            "no-approval",
            "expired",
            "wrong-binding",
            "wrong-env",
            "material-drift",
        ]
        .contains(&case)
        {
            assert!(calls.is_empty());
            assert_eq!(before, bytes(&h::load(&s).unwrap()).unwrap());
        } else {
            assert!(state["pid"].as_u64().is_some_and(|p| p > 0));
            assert!(state["creation"].as_str().is_some_and(|s| !s.is_empty()));
            assert_eq!(state["creation"], state["p_process"]["creation"]);
            assert_eq!(
                state["p_process"]["reclaim"], "CONFIRMED",
                "{case}: {result:?}"
            );
            let before = bytes(&h::load(&s).unwrap()).unwrap();
            let reopened =
                Store::self_open(&h::store_path(&root.join("project")).unwrap()).unwrap();
            assert_eq!(
                run_store(&reopened, None).unwrap_err(),
                "P_NO_APPROVAL_OR_REPLAY"
            );
            assert_eq!(before, bytes(&h::load(&reopened).unwrap()).unwrap());
            assert_eq!(
                calls.len(),
                fs::read_to_string(&trace)
                    .unwrap_or_default()
                    .lines()
                    .count()
            );
        }
        let persisted = String::from_utf8(bytes(&h::load(&s).unwrap()).unwrap()).unwrap();
        assert!(!persisted.contains("SYNTHETIC_SECRET_NOT_FOR_DIAGNOSTICS"));
        assert!(!persisted.contains("userAgent"));
        assert_eq!(
            opens,
            SOURCE.with(|s| s
                .borrow()
                .as_ref()
                .unwrap()
                .starts
                .load(std::sync::atomic::Ordering::SeqCst))
        );
        evidence.push(
            json!({"driver_calls":opens,"case":case,"error":result.err(),"calls":calls.len(),"state":state,"root":root}),
        );
    }
    // Header guard also rejects a turn in every slot and all ninth frames, independently of Adapter.
    for i in 0..9 {
        assert_eq!(
            wire_gate(i, b"{\"id\":1,\"method\":\"turn/start\"}\n"),
            Err("P_FRAME_REJECTED")
        );
    }
    let e = std::env::temp_dir().join(format!("nova-p-probe-evidence-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&e).unwrap();
    fs::write(
        e.join("matrix.json"),
        bytes(&json!({"synthetic_only":true,"cases":evidence})).unwrap(),
    )
    .unwrap();
    println!("P_PROBE_OFFLINE_EVIDENCE={}", e.display());
}
