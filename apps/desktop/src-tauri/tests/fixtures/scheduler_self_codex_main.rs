//! Internal SELF-2 entry. Native admission remains closed; fake operation is explicit.
#[cfg(not(debug_assertions))]
compile_error!("scheduler-validation is debug-only and must never ship");
use nova_lib::scheduler::{
    self_codex as c, self_handoff as h,
    store::{read_file, safe_location, Store},
    Result,
};
use serde_json::{json, Value};
use std::{path::Path, io::{self, IsTerminal, Read, BufRead}};
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
        "permission-project-prepare" if a.len()==1 => return probe::prepare_project(),
        "permission-project-status" if a.len()==1 => return probe::project_status(),
        "permission-project-evaluate" if a.len()==2 => return probe::evaluate_project(&a[1]),
        "permission-prepare" if a.len()==1 => return probe::prepare(),
        "permission-evaluate" if a.len()==2 => return probe::evaluate(&a[1]),
        "permission-run" | "permission-project-run" if a.len()==3 => {
            if !io::stdin().is_terminal() { return Err("CONSOLE_REQUIRED"); }
            eprintln!("NON-MODEL ONLY: restricted-token capability/ACL/NUL initialization; fixed isolated fixture. Type AUTHORIZE NON-MODEL ACL NUL <phase> <digest>:");
            let mut line=String::new();io::stdin().read_line(&mut line).map_err(|_|"INPUT_REJECTED")?;
            if line.len()>256 {return Err("INPUT_BOUND");}
            let invoke=if a[0]=="permission-project-run" {probe::run_project}else{probe::run};
            return invoke(&a[1],&a[2],line.trim_end_matches(['\r','\n']));
        }
        "permission-helper" | "permission-project-helper" if a.len()==3 => {
            let invoke=if a[0]=="permission-project-helper" {probe::helper_project}else{probe::helper};
            let (v,code)=invoke(&a[1],&a[2])?; println!("{v}"); std::process::exit(code);
        }
        x if x.starts_with("permission-") => return Err("PROBE_COMMAND_REJECTED"),
        _ => {}
    }
    if a[0] == "canary-configuration-readiness" && a.len()==1 {return Ok(c::canary_configuration_readiness());}
    match a[0].as_str() {
        "app-server-p-prepare" if a.len()==2=>return c::app_server_p("prepare",read(&a[1])?,""),
        "app-server-p-evaluate" if a.len()==1=>return c::app_server_p("evaluate",Value::Null,""),
        "app-server-p-probe" if a.len()==1=>return c::app_server_p("probe",Value::Null,""),
        "app-server-p-approve" if a.len()==2=>{
            c::app_server_p("check-admission",Value::Null,"")?;
            eprintln!("{}",c::app_server_p("evaluate",Value::Null,"")?);
            if !io::stdin().is_terminal(){return Err("CONSOLE_REQUIRED")}
            eprintln!("P: no user turn; CLI may use authentication/network/cache/trust. Type AUTHORIZE P ZERO TURN 1 SERVER <digest>:");
            let mut line=String::new();io::BufReader::new(io::stdin().take(257)).read_line(&mut line).map_err(|_|"INPUT_REJECTED")?;
            if line.len()>256{return Err("INPUT_BOUND")}
            return c::app_server_p("approve",json!(a[1]),line.trim_end_matches(['\r','\n']));
        },
        _=>{}
    }
    if a[0].starts_with("app-server-") { return c::app_server_admission(); }
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
    if a[0].starts_with("canary-") && Path::new(&a[1])!=nova_lib::scheduler::self_handoff::store_path(&Path::new(probe::PROJECT_ROOT).join("workspace"))? {
        return Err("SELF2_CANARY_FIXED_STORE_REQUIRED");
    }
    let s = Store::self_open(Path::new(&a[1]))?;
    match a[0].as_str() {
        "canary-draft" if a.len()==3 => c::canary_draft(&s,read(&a[2])?),
        "canary-proposal" if a.len()==3 => c::canary_proposal(&s,read(&a[2])?),
        "canary-approve" if a.len()==3 => {
            if !io::stdin().is_terminal(){return Err("CONSOLE_REQUIRED")}
            let request=read(&a[2])?;let proposal=c::canary_proposal(&s,request)?;
            eprintln!("{proposal}");eprintln!("Type AUTHORIZE 1120A CODEX CANARY <digest>:");
            let mut line=String::new();io::stdin().read_line(&mut line).map_err(|_|"INPUT_REJECTED")?;
            if line.len()>256{return Err("INPUT_BOUND")}
            c::approve_canary(&s,read(&a[2])?,line.trim_end_matches(['\r','\n']))
        },
        "canary-run" if a.len()==2 => c::run_canary(&s),
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
            let a: Vec<_> = std::env::args().skip(1).collect();
            let value = if a.first().is_some_and(|v|v.starts_with("permission-")) {
                probe::failure(&a[0],a.get(1).map(String::as_str).unwrap_or(""),e)
            } else if a.first().is_some_and(|v|v.starts_with("app-server-p-")) {
                json!({"error_code":e,"server_started":if e=="P_HOST_TOOL_INITIALIZATION_UNVERIFIED" {"NO"}else{"CONSULT_PERSISTENT_RECEIPTS"},"network_executed":if e=="P_HOST_TOOL_INITIALIZATION_UNVERIFIED" {"NOT_STARTED"}else{"UNKNOWN"},"turn_start_sent":0,"model_authorized":false,"retry":"FORBIDDEN"})
            } else if a.first().is_some_and(|v|v.starts_with("canary-")) {
                json!({"error_code":e,"real_codex_started":"CONSULT_PERSISTENT_RECEIPTS","network_executed":"UNKNOWN","model_admission":"NOT_A_SUCCESS","retry":"FORBIDDEN"})
            } else {
                json!({"error_code":e,"real_codex_started":false,"network_executed":false,"admission":"NOT RUN"})
            };
            println!("{value}");
            std::process::exit(2)
        }
    }
}
