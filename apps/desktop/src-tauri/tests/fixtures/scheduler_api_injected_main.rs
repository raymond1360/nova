#[cfg(not(debug_assertions))]
compile_error!("scheduler-validation is debug-only and must never ship");
use nova_lib::scheduler::{
    contract::*,
    relay,
    store::{Guard, Store},
};
use serde_json::json;
use std::{collections::BTreeMap, path::Path};
fn execute() -> Result<serde_json::Value, &'static str> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().ok_or("SCHEMA_REJECTED")?;
    if !["api-init", "api-run", "api-status"].contains(&cmd.as_str()) {
        return Err("REAL_API_NOT_AUTHORIZED");
    }
    let mut opts = BTreeMap::new();
    let mut i = 1;
    while i < args.len() {
        if !["--workspace", "--plan", "--run-root"].contains(&args[i].as_str())
            || i + 1 >= args.len()
            || opts.insert(args[i].clone(), args[i + 1].clone()).is_some()
        {
            return Err("SCHEMA_REJECTED");
        }
        i += 2;
    }
    let root = Path::new(opts.get("--workspace").ok_or("SCHEMA_REJECTED")?);
    let s = if cmd == "api-init" {
        if opts.len() != 3 {
            return Err("SCHEMA_REJECTED");
        };
        if root.try_exists().map_err(|_| "PATH_REJECTED")? {
            return Err("CREATE_NEW_REJECTED");
        }
        let mut g = Guard::open(
            Path::new(opts.get("--plan").ok_or("SCHEMA_REJECTED")?),
            false,
        )?;
        let b = g.bytes(65536)?;
        let request: ApiPlanRequest = api_json(&b, 65536)?;
        request.validate()?;
        Store::api_init(
            root,
            Path::new(opts.get("--run-root").ok_or("SCHEMA_REJECTED")?),
            &b,
        )?
    } else {
        if opts.len() != 1 {
            return Err("SCHEMA_REJECTED");
        };
        Store::api_open(root)?
    };
    if cmd == "api-run" {
        relay::run_api(&s)
    } else {
        relay::api_status(&s)
    }
}
fn main() {
    match execute() {
        Ok(v) => {
            println!("{}", v);
        }
        Err(e) => {
            println!(
                "{}",
                json!({"schema":"nova.scheduler.api-error/1","mode":"API_INJECTED_ONLY","source":"INJECTED","native_executed":false,"network_executed":false,"execution_authorized":false,"next_execution_authorized":false,"error_code":e})
            );
            std::process::exit(2)
        }
    }
}
