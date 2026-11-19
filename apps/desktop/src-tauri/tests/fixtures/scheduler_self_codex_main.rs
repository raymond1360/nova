//! Internal SELF-2 entry. Native admission remains closed; fake operation is explicit.
#[cfg(not(debug_assertions))]
compile_error!("scheduler-validation is debug-only and must never ship");
use nova_lib::scheduler::{
    self_codex as c, self_handoff as h,
    store::{read_file, safe_location, Store},
    Result,
};
use serde_json::{json, Value};
use std::{path::Path, io::{self, IsTerminal}};
use nova_lib::scheduler::admission_probe as probe;
fn read<T: serde::de::DeserializeOwned>(p: &str) -> Result<T> {
    safe_location(Path::new(p))?;
    serde_json::from_slice(&read_file(Path::new(p), c::INPUT_CAP)?.0).map_err(|_| "INPUT_REJECTED")
}
fn run() -> Result<Value> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if a.is_empty() {
        return Err("SELF2_COMMAND_REJECTED");
    }
    match a[0].as_str() {
        "permission-prepare" if a.len()==1 => return probe::prepare(),
        "permission-evaluate" if a.len()==2 => return probe::evaluate(&a[1]),
        "permission-run" if a.len()==3 => {
            if !io::stdin().is_terminal() { return Err("CONSOLE_REQUIRED"); }
            eprintln!("NON-MODEL ONLY: restricted-token capability/ACL/NUL initialization; fixed isolated fixture. Type AUTHORIZE NON-MODEL ACL NUL <phase> <digest>:");
            let mut line=String::new();io::stdin().read_line(&mut line).map_err(|_|"INPUT_REJECTED")?;
            if line.len()>256 {return Err("INPUT_BOUND");}
            return probe::run(&a[1],&a[2],line.trim_end_matches(['\r','\n']));
        }
        "permission-helper" if a.len()==3 => {
            let (v,code)=probe::helper(&a[1],&a[2])?; println!("{v}"); std::process::exit(code);
        }
        x if x.starts_with("permission-") => return Err("PROBE_COMMAND_REJECTED"),
        _ => {}
    }
    if a[0] == "native-run" {
        c::live_admission()?;
        return Err("SELF2_COMMAND_REJECTED");
    }
    if a[0] == "init-work" && a.len() == 7 {
        let p: c::Plan = read(&a[6])?;
        let input: h::DeliveryInput = read(&a[5])?;
        if serde_json::to_value(&input).map_err(|_| "ENCODING")?
            != serde_json::to_value(&p.input).map_err(|_| "ENCODING")?
        {
            return Err("SELF2_APPROVED_INPUT_MISMATCH");
        }
        let s = h::init_work(
            Path::new(&a[1]),
            Path::new(&a[2]),
            Path::new(&a[3]),
            read(&a[4])?,
            input,
        )?;
        c::attach(&s, p)?;
        return c::status(&s);
    }
    if a.len() < 2 {
        return Err("SELF2_COMMAND_REJECTED");
    }
    let s = Store::self_open(Path::new(&a[1]))?;
    match a[0].as_str() {
        "status" if a.len() == 2 => c::status(&s),
        "attach" if a.len() == 3 => {
            c::attach(&s, read(&a[2])?)?;
            c::status(&s)
        }
        "fake-run" if a.len() == 4 => {
            let mut d = c::FakeCli::new(Path::new(&a[2]), &a[3])?;
            c::run_fake(&s, &mut d)
        }
        "export" if a.len() == 3 => {
            h::export(&s, Path::new(&a[2]))?;
            c::status(&s)
        }
        _ => Err("SELF2_COMMAND_REJECTED"),
    }
}
fn main() {
    match run() {
        Ok(v) => println!("{v}"),
        Err(e) => {
            println!(
                "{}",
                json!({"error_code":e,"real_codex_started":false,"network_executed":false,"admission":"NOT RUN"})
            );
            std::process::exit(2)
        }
    }
}
