//! Debug-only AR0 entry. No real execution switch is available.
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
fn go() -> Result<Value> {
    let mut a = std::env::args().skip(1);
    let command = a.next().ok_or("COMMAND_REJECTED")?;
    let mut kv = BTreeMap::new();
    while let Some(k) = a.next() {
        if !["--workspace", "--source", "--plan", "--run-root"].contains(&k.as_str())
            || kv.insert(k, a.next().ok_or("ARGUMENT_REJECTED")?).is_some()
        {
            return Err("ARGUMENT_REJECTED");
        }
    }
    let workspace = kv.get("--workspace").ok_or("ARGUMENT_REJECTED")?;
    match command.as_str() {
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
                serde_json::to_value(relay::status(&store)?).map_err(|_| "ENCODING")
            } else {
                #[cfg(windows)]
                let mut handler = cli_adapter::ConsoleHandler::install()?;
                let mut driver = cli_adapter::NativeDriver::new()?;
                let outcome = relay::run(&store, &mut driver);
                drop(driver);
                #[cfg(windows)]
                handler.close()?;
                serde_json::to_value(outcome?).map_err(|_| "ENCODING")
            }
        }
        _ => Err("COMMAND_REJECTED"),
    }
}
fn main() {
    match go() {
        Ok(v) => {
            println!("{}", v);
        }
        Err(code) => {
            println!(
                "{}",
                json!({"schema":"nova.relay.error/1","error_code":code,"execution_authorized":false})
            );
            std::process::exit(2)
        }
    }
}
