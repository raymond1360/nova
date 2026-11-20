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
            schema: "nova.self2.permission-probe/2".into(),
            root_identity: Guard::open(&root, true).unwrap().identity,
            nonce: uuid::Uuid::new_v4().to_string(),
            cli_hash: self_codex::CLI_SHA.into(),
            environment: env(&root).unwrap(),
            helper_environment: helper_environment(&root, &env(&root).unwrap()).unwrap(),
            helper: helper.clone(),
            helper_hash: tool_hash(&helper).unwrap(),
            controls: BTreeMap::from([(
                "instructions.txt".into(),
                fact(&root.join("control/instructions.txt")).unwrap(),
            )]),
            targets: current(&root).unwrap(),
            project: None,
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
            fs::write(target_directory(&self.root,&self.p).join("write.txt"), AFTER).unwrap();
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
    fn accepted(&self) -> AcceptedPermission {
        AcceptedPermission { scope: PermissionScope {
            project_root: self.root.join("workspace"), cli: PathBuf::from(r"C:\SYNTHETIC\codex.exe"),
            cli_sha256: self.p.cli_hash.clone(), controller: self.p.helper.clone(),
            controller_sha256: self.p.helper_hash.clone(), permission_plan_sha256: hash(&bytes(&self.p).unwrap()),
        }, receipts: receipt_hashes(&self.root).unwrap() }
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
        assert!(a.contains(&"shell_environment_policy.inherit=\"none\"".into()));
        let set = a.iter().find(|v| v.starts_with("shell_environment_policy.set=")).unwrap();
        for (k, v) in &f.p.environment {
            assert!(set.contains(&format!("{}={}", json!(k), json!(v))));
        }
        assert!(a.contains(&format!("shell_environment_policy.include_only={}",
            json!(f.p.environment.keys().collect::<Vec<_>>()))));
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
    fs::remove_file(f.root.join("home/config.toml")).unwrap();
    assert!(validate(&f.root).is_ok());
    assert!(validate_environment(&f.root, Some(&f.p.helper_environment)).is_ok());
    fs::write(f.root.join("home/config.toml"), b"SYNTHETIC_CONFIG_INJECTION").unwrap();
    assert_eq!(validate_environment(&f.root, Some(&f.p.helper_environment)).unwrap_err(), "PROBE_ENVIRONMENT_DRIFT");
    fs::remove_file(f.root.join("home/config.toml")).unwrap();
    // Never compare the controller map to the helper or reconstruct/overwrite its PATH.
    assert_eq!(validate_environment(&f.root, Some(&f.p.environment)).unwrap_err(), "PROBE_ENVIRONMENT_DRIFT");
    let lower: BTreeMap<_, _> = f.p.helper_environment.iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
    assert!(validate_environment(&f.root, Some(&lower)).is_ok());
    for key in ["CODEX_HOME", "PATH", "PATHEXT", "TEMP", "TMP", "HTTP_PROXY", "SBX_NONET_ACTIVE", "GIT_PAGER"] {
        let mut bad = f.p.helper_environment.clone();
        bad.remove(key);
        assert_eq!(validate_environment(&f.root, Some(&bad)).unwrap_err(), "PROBE_ENVIRONMENT_DRIFT");
        bad.insert(key.into(), "SYNTHETIC_SECRET_DRIFT".into());
        assert_eq!(validate_environment(&f.root, Some(&bad)).unwrap_err(), "PROBE_ENVIRONMENT_DRIFT");
    }
    for key in ["OPENAI_API_KEY", "UNEXPECTED", "path"] {
        let mut bad = f.p.helper_environment.clone();
        bad.insert(key.into(), "SYNTHETIC_SECRET_DRIFT".into());
        assert_eq!(validate_environment(&f.root, Some(&bad)).unwrap_err(), "PROBE_ENVIRONMENT_DRIFT");
    }
    fs::write(f.root.join("home/AGENTS.md"), b"SYNTHETIC_INJECTION").unwrap();
    assert_eq!(validate(&f.root).unwrap_err(), "PROBE_ENVIRONMENT_DRIFT");
    assert_eq!(validate_environment(&f.root, Some(&f.p.helper_environment)).unwrap_err(), "PROBE_ENVIRONMENT_DRIFT");
    fs::remove_file(f.root.join("home/AGENTS.md")).unwrap();
    let mut bad_plan = f.p.clone();
    bad_plan.helper_environment.insert("UNKNOWN".into(), "SYNTHETIC_SECRET_DRIFT".into());
    fs::write(f.root.join("plan.json"), bytes(&bad_plan).unwrap()).unwrap();
    assert_eq!(validate_environment(&f.root, Some(&bad_plan.helper_environment)).unwrap_err(), "PROBE_ENVIRONMENT_DRIFT");
    let mut old_plan = serde_json::to_value(&f.p).unwrap();
    old_plan.as_object_mut().unwrap().remove("helper_environment");
    fs::write(f.root.join("plan.json"), bytes(&old_plan).unwrap()).unwrap();
    assert_eq!(validate(&f.root).unwrap_err(), "PROBE_EVIDENCE_REJECTED");

    // Golden transformations from frozen env.rs, including the Windows PATHEXT fallback.
    let mut host = f.p.environment.clone();
    host.insert("PATHEXT".into(), ".EXE;.bAt;;.COM;.cmd;.BAT".into());
    host.insert("COMPUTERNAME".into(), " /DEV/NULL ".into());
    let profile = Path::new(r"C:\SYNTHETIC_PROFILE");
    let child = helper_environment_for_profile(&f.root, &host, profile).unwrap();
    assert_eq!(child["PATHEXT"], ".bAt;.cmd;.EXE;.COM");
    assert_eq!(child["COMPUTERNAME"], "NUL");
    assert_eq!(child["PATH"], format!(r"C:\SYNTHETIC_PROFILE\.sbx-denybin;{}", host["PATH"]));
    for (k, v) in [("PAGER", "more.com"), ("GIT_PAGER", "more.com"), ("LESS", ""),
        ("HTTP_PROXY", "http://127.0.0.1:9"), ("NO_PROXY", "localhost,127.0.0.1,::1"),
        ("GIT_SSH_COMMAND", "cmd /c exit 1"), ("GIT_ALLOW_PROTOCOLS", ""),
        ("SBX_NONET_ACTIVE", "1"), ("CARGO_NET_OFFLINE", "true")] { assert_eq!(child[k], v); }
    host.remove("PATHEXT");
    assert_eq!(helper_environment_for_profile(&f.root, &host, profile).unwrap()["PATHEXT"], ".BAT;.CMD;.COM;.EXE");
    host.insert("UNKNOWN".into(), "SYNTHETIC_SECRET_DRIFT".into());
    assert_eq!(helper_environment_for_profile(&f.root, &host, profile), Err("PROBE_ENVIRONMENT_DRIFT"));

    // Export only synthetic values for an independent offline TOML parser check.
    let mut serialization = f.p.clone();
    serialization.environment = BTreeMap::from([
        ("SystemRoot".into(), r"C:\SYNTHETIC_WINDOWS".into()),
        ("COMPUTERNAME".into(), "SYNTHETIC \"脛\" \\ line\nnext".into()),
    ]);
    let material = f.root.join("evidence/synthetic-shell-contract.json");
    fs::write(&material, bytes(&json!({
        "host": serialization.environment,
        "argv": argv(&f.root, &serialization, "W")
    })).unwrap()).unwrap();
    println!("SYNTHETIC_SHELL_CONTRACT_FILE={}", material.display());
}
#[test]
fn probe_current_bytes_identity_controls_and_receipts_rechecked() {
    // Full public admission path, ordinary .git directory and declared editable source.
    {
        use super::super::{self_handoff as h, self_codex as c, contract::{Conclusion,ReportType}, store::Store};
        let root=std::env::temp_dir().join(format!("nova-project-producer-{}",uuid::Uuid::new_v4()));
        let helper=std::env::temp_dir().join(format!("synthetic-immutable-helper-{}.exe",uuid::Uuid::new_v4()));
        fs::write(&helper,b"SYNTHETIC IDENTITY ONLY; NEVER EXECUTED").unwrap();
        let sha=tool_hash(&helper).unwrap();
        let prepared=prepare_project_at(&root,&helper,&helper,&sha).unwrap();
        assert_eq!(prepared["accepted"],false);assert_eq!(prepared["model_admission"],false);
        assert!(prepared["write_argv"].to_string().contains("permission-project-helper"));
        assert_eq!(prepare_project_at(&root,&helper,&helper,&sha),Err("PROBE_ROOT_ALREADY_EXISTS"));
        let f=F {p:load(&root.join("plan.json")).unwrap(),root};let workspace=f.root.join("workspace");
        let base=prepared["source_contract"]["base_sha"].as_str().unwrap().to_string();
        let head=prepared["source_contract"]["head_sha"].as_str().unwrap().to_string();
        assert!(f.p.project.is_some());assert!(workspace.join(".git").is_dir());
        f.synthetic("W");f.synthetic("R");let mut accepted=f.accepted();accepted.scope.cli=f.p.helper.clone();
        SYNTHETIC_PENDING.with(|v|*v.borrow_mut()=Some(accepted.scope.clone()));
        struct ClearTrust;impl Drop for ClearTrust {fn drop(&mut self){
            SYNTHETIC_ACCEPTED.with(|v|*v.borrow_mut()=None);SYNTHETIC_PENDING.with(|v|*v.borrow_mut()=None);
        }}
        let _clear=ClearTrust;
        let report=h::tests::report("project-package",&head,ReportType::Development,Conclusion::NotRun);
        let input:h::DeliveryInput=serde_json::from_value(json!({"stage":"project-stage","package":"project-package","base_sha":base,"head_sha":head,
            "goal":"Only edit a.txt","acceptance":["declared source only"],"allowed_paths":["a.txt"],"required_commands":[],"logs":[],"developer_report":report})).unwrap();
        let reports=f.root.join("reports");
        let stage=h::Stage {stage:"project-stage".into(),developer:None,reviewer:None};
        let store=h::init_work(&h::store_path(&workspace).unwrap(),&workspace,&reports,stage,input.clone()).unwrap();
        c::attach(&store,c::Plan {mode:c::Mode::RealPendingAdmission,schema:"nova.self2.plan/1".into(),input,
            project_root:workspace.clone(),cli:accepted.scope.cli.clone(),cli_sha256:accepted.scope.cli_sha256.clone(),
            cli_version:c::CLI_VERSION.into(),model:"SYNTHETIC_NOT_RUN".into(),commands:vec![]}).unwrap();
        let observed=std::cell::RefCell::new(vec![]);
        let admission=|store:&Store| {
            let before=h::load(store).unwrap();let r=c::admission_for(store,&accepted.scope,None);
            let after=h::load(store).unwrap();assert_eq!(before.0,after.0);
            assert_eq!(serde_json::to_value(&before.1).unwrap(),serde_json::to_value(&after.1).unwrap());
            observed.borrow_mut().push(json!({"result":format!("{r:?}"),"revision_before":before.0,"revision_after":after.0,"records":after.1.self2.as_ref().unwrap().records.len(),"state_unchanged":true}));r
        };
        assert_eq!(admission(&store),Err("SELF2_PERMISSION_OBJECT_NOT_ACCEPTED"));
        SYNTHETIC_ACCEPTED.with(|v|*v.borrow_mut()=Some(accepted.clone()));
        assert_eq!(admission(&store),Err("SELF2_MODEL_AUTHORIZATION_REQUIRED"));
        let before=h::load(&store).unwrap();let mut wrong=accepted.scope.clone();
        wrong.permission_plan_sha256="0".repeat(64);
        assert!(c::admission_for(&store,&wrong,None).is_err());
        assert_eq!(serde_json::to_value(h::load(&store).unwrap()).unwrap(),serde_json::to_value(before).unwrap());
        // The accepted scope pins the immutable helper, not this consumer's executable identity.
        assert_eq!(accepted.scope.controller,f.p.helper);
        assert_ne!(accepted.scope.controller,std::env::current_exe().unwrap());
        let mut production_pending=accepted.scope.clone();production_pending.project_root=Path::new(PROJECT_ROOT).join("workspace");
        assert!(matches!(bind_permission(&production_pending),Err("SELF2_PERMISSION_SCOPE_MISMATCH")));
        // A legitimate in-scope source edit is independent of immutable permission sentinels.
        fs::write(workspace.join("a.txt"),b"legal uncommitted model edit\n").unwrap();
        assert_eq!(admission(&store),Err("SELF2_MODEL_AUTHORIZATION_REQUIRED"));
        fs::write(workspace.join("outside.txt"),b"unapproved").unwrap();assert_eq!(admission(&store),Err("SCOPE_REJECTED"));fs::remove_file(workspace.join("outside.txt")).unwrap();
        let config=workspace.join(".git/config");let original=fs::read(&config).unwrap();fs::write(&config,b"[core]\n bare = false\n# drift\n").unwrap();
        assert!(admission(&store).is_err());fs::write(&config,original).unwrap();
        let marker=workspace.join(PROJECT_TARGETS).join("readonly.txt");fs::write(&marker,b"drift").unwrap();assert!(admission(&store).is_err());fs::write(&marker,BEFORE).unwrap();
        let receipt=f.root.join("evidence/R.exit.json");let raw=fs::read(&receipt).unwrap();let mut unknown:Exit=load(&receipt).unwrap();unknown.execution.reclaim="UNKNOWN".into();fs::write(&receipt,bytes(&unknown).unwrap()).unwrap();assert!(admission(&store).is_err());
        // Even test authority accepting the edited hashes cannot bypass original cleanup/UNKNOWN checks.
        let mut updated=f.accepted();updated.scope.cli=accepted.scope.cli.clone();SYNTHETIC_ACCEPTED.with(|v|*v.borrow_mut()=Some(updated));assert!(admission(&store).is_err());
        fs::write(&receipt,raw).unwrap();SYNTHETIC_ACCEPTED.with(|v|*v.borrow_mut()=Some(accepted.clone()));assert_eq!(admission(&store),Err("SELF2_MODEL_AUTHORIZATION_REQUIRED"));
        // Existing Store record state blocks admission without clearing or replaying it.
        for outcome in ["STARTED","UNKNOWN"] {
            let (rev,mut state)=h::load(&store).unwrap();state.self2.as_mut().unwrap().records=vec![c::Record {key:"synthetic-reserved".into(),
                attempt:serde_json::from_value(json!({"attempt_id":uuid::Uuid::new_v4().to_string(),"role":"DEVELOP","state":"LAUNCHING","state_version":0,"input_identity":"0".repeat(64),"profile_id":"SELF2_DEVELOP","exe_sha256":"0".repeat(64),"argv_sha256":"0".repeat(64),"env_names_sha256":"0".repeat(64)})).unwrap(),
                spec_sha256:"0".repeat(64),expected_uuid:None,outcome:outcome.into(),execution:None,stdout_path:None,stderr_path:None}];h::save(&store,rev,&state).unwrap();
            assert_eq!(admission(&store),Err("SELF2_RECONCILE_REQUIRED"));
        }
        let trace=f.root.join("reports/public-admission-evidence.json");
        fs::write(&trace,bytes(&json!({"synthetic_only":true,"entry":"self_codex::admission_for","mode":"RealPendingAdmission","project":workspace,"permission_schema":f.p.schema,"git_is_local_directory":workspace.join(".git").is_dir(),"observations":observed.into_inner()})).unwrap()).unwrap();
        println!("SYNTHETIC_PUBLIC_ADMISSION_EVIDENCE={}",trace.display());
    }
    // Test-only accepted anchors; production accepts only the pinned 1119e raw receipts.
    let a=F::new();a.synthetic("W");a.synthetic("R");let accepted=a.accepted();
    let binding=permission_at(&a.root,&accepted.scope,&accepted,self_codex::CLI_SHA).unwrap();
    assert_eq!(self_codex::finish_admission(Some(&binding),None,"synthetic-plan"),Err("SELF2_MODEL_AUTHORIZATION_REQUIRED"));
    self_codex::test_model_authorization_binding(&binding);
    for key in ["root","cli","controller","cli_hash","controller_hash","configuration"] {
        let mut scope=accepted.scope.clone();
        match key { "root"=>scope.project_root=a.root.join("other"),"cli"=>scope.cli=a.root.join("other.exe"),
            "controller"=>scope.controller=a.root.join("other.exe"),"cli_hash"=>scope.cli_sha256="0".repeat(64),
            "controller_hash"=>scope.controller_sha256="0".repeat(64),_=>scope.permission_plan_sha256="0".repeat(64) }
        assert!(permission_at(&a.root,&scope,&accepted,self_codex::CLI_SHA).is_err());
        assert!(bind_permission(&scope).is_err()); // rejected before touching the real fixed fixture.
    }
    assert!(permission_at(&a.root,&accepted.scope,&accepted,&"0".repeat(64)).is_err());
    let receipt=a.root.join("evidence/R.exit.json");let original=fs::read(&receipt).unwrap();
    let mut bad: Exit=load(&receipt).unwrap();bad.execution.reclaim="UNKNOWN".into();
    fs::write(&receipt,bytes(&bad).unwrap()).unwrap();
    assert!(permission_at(&a.root,&accepted.scope,&accepted,self_codex::CLI_SHA).is_err());
    let recomputed=a.accepted(); // even self-consistent edited hashes cannot turn UNKNOWN into success.
    assert!(permission_at(&a.root,&recomputed.scope,&recomputed,self_codex::CLI_SHA).is_err());
    fs::write(&receipt,&original).unwrap();
    fs::write(a.root.join("workspace/write.txt"),b"EXPIRED_APPLICABILITY").unwrap();
    assert!(permission_at(&a.root,&accepted.scope,&accepted,self_codex::CLI_SHA).is_err());
    fs::write(a.root.join("workspace/write.txt"),AFTER).unwrap();
    assert!(permission_at(&a.root,&accepted.scope,&accepted,self_codex::CLI_SHA).is_ok());
    fs::remove_file(&receipt).unwrap();
    assert!(permission_at(&a.root,&accepted.scope,&accepted,self_codex::CLI_SHA).is_err());
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
        helper_at(&f.root, "W", "wrong", &f.p.helper_environment),
        Err("PROBE_NONCE_REJECTED")
    );
    let (v, exit) = helper_at(&f.root, "W", &f.p.nonce, &f.p.helper_environment).unwrap();
    assert_eq!(exit, 0);
    assert_eq!(v["outcome"], "WROTE");
    assert_eq!(current(&f.root).unwrap(), expected(&f.p, "W", true));
    assert_eq!(
        helper_at(&f.root, "W", &f.p.nonce, &f.p.helper_environment),
        Err("PROBE_CURRENT_FILE_DRIFT")
    );
    let s = f.start("R");
    reserve(&f.root, "R", &s).unwrap();
    let (v, exit) = helper_at(&f.root, "R", &f.p.nonce, &f.p.helper_environment).unwrap();
    assert_eq!(exit, 24);
    assert_eq!(v["outcome"], "UNEXPECTED_WRITE_OPEN");
    assert_eq!(
        fs::read(f.root.join("workspace/readonly.txt")).unwrap(),
        BEFORE
    );
    assert!(helper_at(&f.root, "../other", &f.p.nonce, &f.p.helper_environment).is_err());
}
