//! Offline-only SELF-1 CLI. Reads explicit JSON; never launches Codex or runs commands.
#[cfg(not(debug_assertions))]
compile_error!("scheduler-validation is debug-only and must never ship");
use nova_lib::scheduler::{
    contract::Report,
    self_handoff::*,
    store::{read_file, Store},
    Result,
};
use serde_json::{json, Value};
use std::path::Path;
fn read<T: serde::de::DeserializeOwned>(p: &str) -> Result<T> {
    nova_lib::scheduler::store::safe_location(Path::new(p))?;
    let b = read_file(Path::new(p), 2 * 1024 * 1024)?.0;
    serde_json::from_slice(&b).map_err(|_| "INPUT_REJECTED")
}
fn run() -> Result<Value> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if a.len() < 2 {
        return Err("SELF_COMMAND_REJECTED");
    }
    if a[0] == "init" && a.len() == 6 {
        let s = init(
            Path::new(&a[1]),
            Path::new(&a[2]),
            Path::new(&a[3]),
            read(&a[4])?,
            read(&a[5])?,
        )?;
        return summary(&s);
    }
    if a[0] == "store-path" && a.len() == 2 {
        return Ok(
            json!({"canonical_store":store_path(Path::new(&a[1]))?,"external_action_executed":false}),
        );
    }
    let s = Store::self_open(Path::new(&a[1]))?;
    match a[0].as_str() {
        "status" if a.len() == 2 => return summary(&s),
        "collect" if a.len() == 3 => collect(&s, read(&a[2])?)?,
        "bind-developer" if a.len() == 4 => bind_developer(&s, &a[2], read(&a[3])?)?,
        "bind" if a.len() == 4 => bind(&s, &a[2], read(&a[3])?)?,
        "route" if a.len() == 4 => return route(&s, &a[2], &a[3]),
        "review" if a.len() == 3 => review(&s, read(&a[2])?)?,
        "repair" if a.len() == 5 => approve_repair(&s, &a[2], &a[3], read::<Report>(&a[4])?)?,
        "uncertain" if a.len() == 5 => uncertain(&s, &a[2], &a[3], &a[4])?,
        "close" if a.len() == 5 => close(&s, &a[2], &a[3], read::<Report>(&a[4])?)?,
        "export" if a.len() == 3 => export(&s, Path::new(&a[2]))?,
        _ => return Err("SELF_COMMAND_REJECTED"),
    };
    summary(&s)
}
fn main() {
    match run() {
        Ok(v) => println!("{v}"),
        Err(e) => {
            println!(
                "{}",
                json!({"error_code":e,"real_codex_started":false,"execution_authorized":false})
            );
            std::process::exit(2)
        }
    }
}
