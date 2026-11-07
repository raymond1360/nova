//! Debug-only AR0 entry. Only the approval-bound pilot command can obtain a real permit.
#[cfg(not(debug_assertions))]
compile_error!("scheduler-validation is debug-only and must never ship");
use nova_lib::scheduler::{
    cli_adapter,
    contract::*,
    relay,
    store::{read_file, Store},
    Result,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};
fn go(origin: std::time::Instant) -> Result<Value> {
    let mut a = std::env::args().skip(1);
    let command = a.next().ok_or("COMMAND_REJECTED")?;
    let mut kv = BTreeMap::new();
    while let Some(k) = a.next() {
        if ![
            "--workspace",
            "--source",
            "--plan",
            "--run-root",
            "--approval",
            "--build-receipt",
        ]
        .contains(&k.as_str())
            || kv.insert(k, a.next().ok_or("ARGUMENT_REJECTED")?).is_some()
        {
            return Err("ARGUMENT_REJECTED");
        }
    }
    let workspace = kv.get("--workspace").ok_or("ARGUMENT_REJECTED")?;
    match command.as_str() {
        "relay-pilot-once" => Ok(pilot(&kv, origin)),
        "relay-init" => {
            if kv.len() != 4 {
                return Err("ARGUMENT_REJECTED");
            }
            let (bytes, _) = read_file(
                Path::new(kv.get("--plan").ok_or("ARGUMENT_REJECTED")?),
                65536,
            )?;
            let p: RelayPlan = relay_json(&bytes, 65536)?;
            if !p.mock {
                return Err("REAL_PILOT_NOT_AUTHORIZED");
            }
            p.validate()?;
            let source = Store::open(Path::new(kv.get("--source").ok_or("ARGUMENT_REJECTED")?))?;
            // A source is a fresh V1 sampling registration, never a V2 attempt history to clone/reset.
            if source.relay_require_v2() != Err("RELAY_SCHEMA_UNSUPPORTED") {
                return Err("SOURCE_REGISTRATION_NOT_FRESH");
            }
            let source_snapshot = source.snapshot()?;
            if source_snapshot.revision != 0 {
                return Err("SOURCE_REGISTRATION_NOT_FRESH");
            }
            let m = source_snapshot.manifest;
            let e = source.audit_envelope()?;
            if e.head != p.input_base_sha
                || e.delivery_identity != p.input_delivery_identity
                || p.registration_revision != 0
            {
                return Err("BINDING_MISMATCH");
            }
            relay::read_base(&source, &p)?;
            let owned = Store::relay_init(Path::new(workspace), &m, &e)?;
            let plan = owned.relay_bind(
                &p,
                Path::new(kv.get("--run-root").ok_or("ARGUMENT_REJECTED")?),
            )?;
            Ok(
                json!({"schema":"nova.relay.init/1","plan_id":plan.id,"mock":true,"execution_authorized":false}),
            )
        }
        "relay-status" | "relay-run" => {
            if kv.len() != 1 {
                return Err("ARGUMENT_REJECTED");
            }
            let store = Store::open(Path::new(workspace))?;
            store.relay_require_v2()?;
            if command == "relay-status" {
                if store.relay_plan()?.mode.is_real() {
                    relay::status_real(&store)
                } else {
                    serde_json::to_value(relay::status(&store)?).map_err(|_| "ENCODING")
                }
            } else {
                if store.relay_plan()?.mode.is_real() {
                    return Err("REAL_PILOT_NOT_AUTHORIZED");
                }
                #[cfg(windows)]
                let mut handler = cli_adapter::ConsoleHandler::install()?;
                let mut driver = cli_adapter::NativeDriver::new()?;
                let outcome = relay::run(&store, &mut driver);
                drop(driver);
                #[cfg(windows)]
                let close = handler.close();
                #[cfg(not(windows))]
                let close = Ok(());
                relay::finish_run(
                    &store,
                    outcome.and_then(|v| serde_json::to_value(v).map_err(|_| "ENCODING")),
                    close,
                )
            }
        }
        _ => Err("COMMAND_REJECTED"),
    }
}
fn main() {
    let origin = std::time::Instant::now();
    match go(origin) {
        Ok(v) => {
            println!("{}", v);
            if v["schema"] == "nova.relay.run-outcome/1" {
                std::process::exit(3)
            }
            if v.get("error_code").is_some() {
                std::process::exit(2)
            }
        }
        Err(code) => {
            let value = if std::env::args().nth(1).as_deref() == Some("relay-pilot-once") {
                json!({"schema":"nova.relay.error/1","error_code":code,"next_execution_authorized":false})
            } else {
                json!({"schema":"nova.relay.error/1","error_code":code,"execution_authorized":false})
            };
            println!("{}", value);
            std::process::exit(2)
        }
    }
}

/// No defaults and no program/model/auth override. Missing approval fails before any tool read.
fn pilot(kv: &BTreeMap<String, String>, origin: std::time::Instant) -> Value {
    let mut workspace_created = false;
    let mut run_created = false;
    let mut tool_reads_started = false;
    #[cfg(windows)]
    let mut pilot_handler = None::<cli_adapter::ConsoleHandler>;
    let outcome = (|| -> Result<Value> {
        if kv.len() != 6
            || ![
                "--source",
                "--workspace",
                "--plan",
                "--approval",
                "--build-receipt",
                "--run-root",
            ]
            .iter()
            .all(|k| kv.contains_key(*k))
        {
            return Err("ARGUMENT_REJECTED");
        }
        let (a_raw, _) =
            read_file(Path::new(&kv["--approval"]), 16384).map_err(|_| "APPROVAL_MISSING")?;
        let approval: RealPilotApproval = relay_json(&a_raw, 16384)?;
        // Time precedes even plan/receipt reads: the negative expired fixture has no executable identity.
        approval.check_time(chrono::Utc::now())?;
        let (core_raw, _) = read_file(Path::new(&kv["--plan"]), 65536)?;
        let core: RealPlanCore = relay_json(&core_raw, 65536)?;
        approval.validate(&core)?;
        if approval.roots.workspace != kv["--workspace"]
            || approval.roots.run_root != kv["--run-root"]
            || approval.roots.source_registration != kv["--source"]
        {
            return Err("APPROVAL_MISMATCH");
        }
        for key in ["--workspace", "--run-root"] {
            let p = Path::new(&kv[key]);
            if p.try_exists().map_err(|_| "PATH_REJECTED")? {
                return Err("CREATE_NEW_REJECTED");
            }
            let _ =
                nova_lib::scheduler::store::Guard::open(p.parent().ok_or("PATH_REJECTED")?, true)?;
        }
        let (b_raw, _) = read_file(Path::new(&kv["--build-receipt"]), 16384)?;
        let receipt: BuildReceipt = relay_json(&b_raw, 16384)?;
        #[cfg(windows)]
        {
            pilot_handler = Some(cli_adapter::ConsoleHandler::install()?);
        }
        tool_reads_started = true;
        let mut permit =
            relay::authorize_real_once(core, approval, &a_raw, receipt, &b_raw, origin)?;
        let mut driver = cli_adapter::RealDriver::new(&permit)?;
        permit.qualify(&mut driver)?;
        if permit
            .end()
            .saturating_duration_since(std::time::Instant::now())
            < std::time::Duration::from_secs(60)
        {
            return Err("BUDGET_EXHAUSTED");
        }
        if cli_adapter::cancel_requested() {
            return Err("CANCELLED");
        }
        let source = Store::open(Path::new(&kv["--source"]))?;
        if source.relay_require_v2() != Err("RELAY_SCHEMA_UNSUPPORTED") {
            return Err("SOURCE_REGISTRATION_NOT_FRESH");
        }
        let snap = source.snapshot()?;
        if snap.revision != 0 {
            return Err("SOURCE_REGISTRATION_NOT_FRESH");
        }
        let env = source.audit_envelope()?;
        relay::read_base(&source, &permit.record().core.common())?;
        let store = Store::relay_init_real(
            Path::new(&kv["--workspace"]),
            &snap.manifest,
            &env,
            &permit,
            &mut workspace_created,
        )?;
        store.relay_bind_real_tracked(&permit, &mut run_created)?;
        let result = relay::run_real_once(&store, &mut driver, permit);
        drop(driver);
        result
    })();
    let value = match outcome {
        Ok(v) => v,
        Err(code) => {
            let mut residuals = vec![];
            if workspace_created {
                residuals.push("WORKSPACE");
            }
            if run_created {
                residuals.push("RUN_ROOT");
            }
            json!({"schema":"nova.relay.error/1","error_code":code,"residuals":residuals,"tool_reads_started":tool_reads_started,"next_execution_authorized":false})
        }
    };
    #[cfg(windows)]
    if let Some(mut handler) = pilot_handler {
        if handler.close().is_err() {
            if let Ok(store) = Store::open(Path::new(&kv["--workspace"])) {
                return relay::post_run_failure(&store, value);
            }
            return json!({"schema":"nova.relay.run-outcome/1","outcome":value,"post_run_errors":["HANDLER_UNREGISTER"],"next_action":"PM_REQUIRED","next_execution_authorized":false,"persisted":false});
        }
    }
    value
}
