use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
struct F {
    root: PathBuf,
    p: Plan,
}
impl F {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("nova-probe-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        for n in ["workspace", "control", "evidence", "home", "tmp"] {
            fs::create_dir(root.join(n)).unwrap();
        }
        for n in ["write.txt", "readonly.txt"] {
            fs::write(root.join("workspace").join(n), BEFORE).unwrap();
        }
        fs::write(root.join("control/instructions.txt"), b"SYNTHETIC ONLY").unwrap();
        let helper = std::env::current_exe().unwrap();
        let p = Plan {
            schema: "nova.self2.permission-probe/1".into(),
            root_identity: Guard::open(&root, true).unwrap().identity,
            nonce: uuid::Uuid::new_v4().to_string(),
            cli_hash: self_codex::CLI_SHA.into(),
            environment: env(&root).unwrap(),
            helper: helper.clone(),
            helper_hash: tool_hash(&helper).unwrap(),
            controls: BTreeMap::from([(
                "instructions.txt".into(),
                fact(&root.join("control/instructions.txt")).unwrap(),
            )]),
            targets: current(&root).unwrap(),
        };
        fresh(&root.join("plan.json"), &p).unwrap();
        Self { root, p }
    }
    fn start(&self, step: &str) -> Start {
        Start {
            digest: hash(&bytes(&self.p).unwrap()),
            phase: step.into(),
            command_hash: hash(&bytes(&argv(&self.root, &self.p, step)).unwrap()),
            before: expected(&self.p, step, false),
            host_write_allowed: true,
        }
    }
    fn synthetic(&self, step: &str) {
        let s = self.start(step);
        reserve(&self.root, step, &s).unwrap();
        if step == "W" {
            fs::write(self.root.join("workspace/write.txt"), AFTER).unwrap();
        }
        let mut e = Execution::failed(if step == "W" {
            "SUCCESS"
        } else {
            "RESULT_ERROR"
        });
        e.created = true;
        e.pid = Some(123);
        e.process_creation_utc = Some("SYNTHETIC_TIME".into());
        e.natural = true;
        e.exit_code = Some(if step == "W" { 0 } else { 23 });
        e.stdout = bytes(&ToolReceipt {
            schema: "nova.self2.permission-tool/1".into(),
            nonce: self.p.nonce.clone(),
            phase: step.into(),
            target: target(step).into(),
            outcome: if step == "W" { "WROTE" } else { "DENIED" }.into(),
            win32_error: if step == "W" { 0 } else { 5 },
        })
        .unwrap();
        fresh(
            &self
                .root
                .join("evidence")
                .join(format!("{step}.process.json")),
            &json!({"pid":123,"creation":"SYNTHETIC_TIME"}),
        )
        .unwrap();
        fresh(
            &self.root.join("evidence").join(format!("{step}.exit.json")),
            &Exit {
                start_hash: hash(&bytes(&s).unwrap()),
                cleanup: Some(super::super::contract::CleanupReceipt {
                    root_exited_before: true,
                    active_before: Some(0),
                    terminated: false,
                    job_confirmed: true,
                    readers_finished: true,
                }),
                execution: e,
            },
        )
        .unwrap();
    }
    fn driver(&self, case: &str) -> (NativeDriver, Command) {
        let exe = std::env::current_exe().unwrap();
        let sha = tool_hash(&exe).unwrap();
        let d = NativeDriver::self2_transport(&exe, &sha).unwrap();
        let mut environment = env(&self.root).unwrap();
        environment.insert("NOVA_PERMISSION_FAKE_CASE".into(), case.into());
        let s = Spec {
            role: RelayRole::Develop,
            profile: "SELF2_PERMISSION_ONLY".into(),
            exe,
            exe_sha256: sha,
            argv: vec![
                "--exact".into(),
                "scheduler::admission_probe::tests::probe_fake_child".into(),
                "--nocapture".into(),
            ],
            env: environment,
            cwd: self.root.join("workspace"),
            session: String::new(),
        };
        (d, Command { spec: s })
    }
}
// Keep failure evidence instead of deleting synthetic directories.
#[test]
fn probe_fake_child() {
    if let Ok(c) = std::env::var("NOVA_PERMISSION_FAKE_CASE") {
        match c.as_str() {
            "success" => {
                println!("SYNTHETIC_PERMISSION_RECEIPT");
                std::process::exit(0)
            }
            "fail" => std::process::exit(7),
            "wait" => {
                std::thread::sleep(Duration::from_secs(20));
                std::process::exit(9)
            }
            _ => panic!("unknown synthetic case"),
        }
    }
}
#[test]
fn probe_fixed_surface_and_live_gate_closed() {
    assert!(self_codex::live_admission().is_err());
    assert_eq!(phase("exec"), Err("PROBE_PHASE_REJECTED"));
    let f = F::new();
    let a = argv(&f.root, &f.p, "R");
    assert_eq!(a[0], "sandbox");
    assert!(a.contains(&"windows.sandbox=\"unelevated\"".into()));
    for (phase, profile) in [("W", ":workspace"), ("R", ":read-only")] {
        let a = argv(&f.root, &f.p, phase);
        let index = a.iter().position(|x| x == "--permission-profile").unwrap();
        assert_eq!(a[index + 1], profile);
        assert!(a.contains(&"--include-managed-config".into()));
        assert!(a.contains(&"sandbox_workspace_write.network_access=false".into()));
        assert!(!a.iter().any(|x| x.starts_with("sandbox_mode=")
            || x == "--sandbox-state-json"
            || x == "--sandbox-state-disable-network"));
        assert_eq!(&a[a.len() - 3..], &["permission-helper", phase, &f.p.nonce]);
    }
    assert!(!a
        .iter()
        .any(|a| ["exec", "resume", "login", "uninstall"].contains(&a.as_str())));
    assert_eq!(
        run("W", "bad", "bad"),
        Err("PROBE_EXPLICIT_APPROVAL_REQUIRED")
    );
    fs::write(
        f.root.join("home/config.toml"),
        b"SYNTHETIC_CONFIG_INJECTION",
    )
    .unwrap();
    assert_eq!(validate(&f.root).unwrap_err(), "PROBE_ENVIRONMENT_DRIFT");
}
#[test]
fn probe_current_bytes_identity_controls_and_receipts_rechecked() {
    let f = F::new();
    f.synthetic("W");
    assert!(check(&f.root, "W").is_ok());
    let observation = failure_at(&f.root, "permission-evaluate", "W", "SYNTHETIC_ONLY");
    assert_eq!(
        observation["sandbox_helper_execution"],
        "FIXED_TOOL_RECEIPT_OBSERVED_NOT_ADMISSION"
    );
    assert_eq!(
        observation["attempt_observation"]["cli_process_started"],
        true
    );
    assert_eq!(observation["model_started"], false);

    f.synthetic("R");
    assert!(check(&f.root, "R").is_ok());
    fs::write(f.root.join("workspace/readonly.txt"), b"DRIFT").unwrap();
    assert_eq!(check(&f.root, "W").unwrap_err(), "PROBE_CURRENT_FILE_DRIFT");
    fs::write(f.root.join("workspace/readonly.txt"), BEFORE).unwrap();
    fs::write(f.root.join("control/instructions.txt"), b"DRIFT").unwrap();
    assert_eq!(check(&f.root, "R").unwrap_err(), "PROBE_CONTROL_DRIFT");
    let g = F::new();
    g.synthetic("W");
    fs::rename(
        g.root.join("workspace/write.txt"),
        g.root.join("evidence/old-write.txt"),
    )
    .unwrap();
    fs::write(g.root.join("workspace/write.txt"), AFTER).unwrap();
    assert_eq!(check(&g.root, "W").unwrap_err(), "PROBE_CURRENT_FILE_DRIFT");
    let h = F::new();
    h.synthetic("W");
    let p = h.root.join("evidence/W.exit.json");
    let mut x: Exit = load(&p).unwrap();
    x.execution.stdout = b"model says denied".to_vec();
    fs::write(p, bytes(&x).unwrap()).unwrap();
    assert!(check(&h.root, "W").is_err());
    let x = F::new();
    x.synthetic("W");
    fs::write(x.root.join("workspace/write.txt"), BEFORE).unwrap();
    let receipt = x.root.join("evidence/W.exit.json");
    let mut exit: Exit = load(&receipt).unwrap();
    exit.execution.exit_code = Some(2);
    exit.execution.terminal = "RESULT_ERROR".into();
    exit.execution.stdout.clear();
    exit.execution.stderr = MISSING_PROFILE_STDERR.as_bytes().to_vec();
    fs::write(&receipt, bytes(&exit).unwrap()).unwrap();
    assert_eq!(check(&x.root, "W").unwrap_err(), "PROBE_CLI_EXIT_REJECTED");
    let detail = failure_at(
        &x.root,
        "permission-evaluate",
        "W",
        "PROBE_CLI_EXIT_REJECTED",
    );
    assert_eq!(detail["attempt_observation"]["cli_process_started"], true);
    assert_eq!(
        detail["attempt_observation"]["process_identity_matches"],
        true
    );
    assert_eq!(detail["attempt_observation"]["exit_code"], 2);
    assert_eq!(
        detail["attempt_observation"]["safe_stderr"],
        MISSING_PROFILE_STDERR
    );
    assert_eq!(detail["model_started"], false);
    assert_eq!(
        detail["sandbox_helper_execution"],
        "ARGUMENT_REJECTION_BEFORE_DISPATCH_INFERRED_FROM_CLI"
    );
    assert!(detail.get("real_codex_started").is_none());
    fs::write(
        x.root.join("evidence/W.process.json"),
        b"{\"pid\":999,\"creation\":\"SYNTHETIC_TIME\"}",
    )
    .unwrap();
    assert_eq!(
        check(&x.root, "W").unwrap_err(),
        "PROBE_PROCESS_RECEIPT_REJECTED"
    );

    fs::write(
        x.root.join("evidence/W.process.json"),
        b"{\"pid\":123,\"creation\":\"WRONG_CREATION\"}",
    )
    .unwrap();
    assert_eq!(
        check(&x.root, "W").unwrap_err(),
        "PROBE_PROCESS_RECEIPT_REJECTED"
    );
    exit.execution
        .stderr
        .extend_from_slice(b"SYNTHETIC_SECRET_MUST_NOT_APPEAR");
    fs::write(&receipt, bytes(&exit).unwrap()).unwrap();
    let detail = failure_at(
        &x.root,
        "permission-run",
        "W",
        "PROBE_PROCESS_RECEIPT_REJECTED",
    );
    assert!(detail["attempt_observation"]["safe_stderr"].is_null());
    assert!(!detail
        .to_string()
        .contains("SYNTHETIC_SECRET_MUST_NOT_APPEAR"));
    let empty = F::new();
    let detail = failure_at(&empty.root, "permission-run", "W", "PROBE_EVIDENCE_UNKNOWN");
    assert!(detail["attempt_observation"]["cli_process_started"].is_null());
}
#[test]
fn probe_native_success_failure_start_failure_and_no_replay() {
    for case in ["success", "fail", "start-fail"] {
        let f = F::new();
        let (mut d, mut c) = f.driver(if case == "start-fail" {
            "success"
        } else {
            case
        });
        if case == "start-fail" {
            c.spec.cwd = f.root.join("missing");
        }
        let s = f.start("W");
        capture(
            &f.root,
            "W",
            &s,
            &c,
            &mut d,
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        let e: Exit = load(&f.root.join("evidence/W.exit.json")).unwrap();
        assert_eq!(e.execution.reclaim, "CONFIRMED");
        if case == "start-fail" {
            assert!(!e.execution.created);
        } else {
            assert!(e.execution.natural);
            assert_eq!(
                e.execution.exit_code,
                Some(if case == "success" { 0 } else { 7 })
            );
            assert!(e.execution.pid.is_some());
        }
        assert_eq!(
            capture(
                &f.root,
                "W",
                &s,
                &c,
                &mut d,
                Instant::now() + Duration::from_secs(1)
            ),
            Err("PROBE_ALREADY_RESERVED")
        );
        assert_eq!(current(&f.root).unwrap(), f.p.targets);
    }
}
#[test]
fn probe_native_timeout_cancel_unknown_never_replay() {
    for case in ["timeout", "cancel", "unknown"] {
        let f = F::new();
        let (mut d, c) = f.driver(if case == "unknown" { "success" } else { "wait" });
        let cancel: Arc<AtomicBool> = d.cancel.clone();
        let t = if case == "cancel" {
            Some(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                cancel.store(true, Ordering::Release);
            }))
        } else {
            None
        };
        if case == "unknown" {
            d.fault = Some("reclaim");
        }
        let end = Instant::now()
            + if case == "timeout" {
                Duration::from_millis(250)
            } else {
                Duration::from_secs(10)
            };
        let s = f.start("W");
        capture(&f.root, "W", &s, &c, &mut d, end).unwrap();
        if let Some(t) = t {
            t.join().unwrap();
        }
        let e: Exit = load(&f.root.join("evidence/W.exit.json")).unwrap();
        if case == "unknown" {
            assert_eq!(e.execution.reclaim, "UNKNOWN");
        } else {
            assert!(!e.execution.natural);
            assert_eq!(e.execution.reclaim, "CONFIRMED");
            assert!(e.cleanup.as_ref().unwrap().job_confirmed);
        }
        assert_eq!(
            capture(
                &f.root,
                "W",
                &s,
                &c,
                &mut d,
                Instant::now() + Duration::from_secs(1)
            ),
            Err("PROBE_ALREADY_RESERVED")
        );
        assert_eq!(current(&f.root).unwrap(), f.p.targets);
    }
}
#[test]
fn probe_started_without_receipt_never_replays() {
    let f = F::new();
    let (mut d, c) = f.driver("success");
    let s = f.start("W");
    reserve(&f.root, "W", &s).unwrap();
    assert!(check(&f.root, "W").is_err());
    assert_eq!(
        capture(
            &f.root,
            "W",
            &s,
            &c,
            &mut d,
            Instant::now() + Duration::from_secs(1)
        ),
        Err("PROBE_ALREADY_RESERVED")
    );
    assert!(!f.root.join("evidence/W.process.json").exists());
}

#[test]
fn probe_helper_only_owned_existing_targets_exact_bytes() {
    let f = F::new();
    let s = f.start("W");
    reserve(&f.root, "W", &s).unwrap();
    assert_eq!(
        helper_at(&f.root, "W", "wrong"),
        Err("PROBE_NONCE_REJECTED")
    );
    let (v, exit) = helper_at(&f.root, "W", &f.p.nonce).unwrap();
    assert_eq!(exit, 0);
    assert_eq!(v["outcome"], "WROTE");
    assert_eq!(current(&f.root).unwrap(), expected(&f.p, "W", true));
    assert_eq!(
        helper_at(&f.root, "W", &f.p.nonce),
        Err("PROBE_CURRENT_FILE_DRIFT")
    );
    let s = f.start("R");
    reserve(&f.root, "R", &s).unwrap();
    let (v, exit) = helper_at(&f.root, "R", &f.p.nonce).unwrap();
    assert_eq!(exit, 24);
    assert_eq!(v["outcome"], "UNEXPECTED_WRITE_OPEN");
    assert_eq!(
        fs::read(f.root.join("workspace/readonly.txt")).unwrap(),
        BEFORE
    );
    assert!(helper_at(&f.root, "../other", &f.p.nonce).is_err());
}
