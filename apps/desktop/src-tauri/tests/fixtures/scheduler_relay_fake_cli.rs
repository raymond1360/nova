//! Fixed self-owned protocol fixture. Never invokes an AI or arbitrary executable.
#[cfg(not(debug_assertions))]
compile_error!("scheduler-validation is debug-only and must never ship");
use nova_lib::scheduler::{
    cli_adapter::{self, argv, proposal_schema},
    contract::*,
};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::Duration,
};
fn emit(v: Value) {
    println!("{}", v);
    std::io::stdout().flush().unwrap();
}
fn main() {
    if let Err(c) = go() {
        eprintln!("{}", c);
        std::process::exit(3)
    }
}
fn go() -> Result<(), &'static str> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a == ["--env-controller"] {
        return cli_adapter::isolated_environment_check();
    }
    if a == ["--descendant"] {
        loop {
            std::thread::sleep(Duration::from_secs(60))
        }
    }
    if a == ["--cancel-controller-c"] || a == ["--cancel-controller-break"] {
        return cli_adapter::isolated_console_check(a[0].ends_with("break"));
    }
    let role = if a.first().is_some_and(|s| s == "--no-daemon") {
        RelayRole::Develop
    } else {
        RelayRole::Review
    };
    let cwd = std::env::current_dir().map_err(|_| "CWD")?;
    let root = cwd.parent().ok_or("CWD")?;
    let model_key = if role == RelayRole::Develop {
        "-m"
    } else {
        "--model"
    };
    let model = a
        .windows(2)
        .find(|v| v[0] == model_key)
        .map(|v| v[1].as_str())
        .ok_or("ARGV")?;
    let session = if role == RelayRole::Review {
        a.windows(2)
            .find(|v| v[0] == "--session-id")
            .map(|v| v[1].clone())
            .ok_or("ARGV")?
    } else {
        "fixed-thread".into()
    };
    if !cli_adapter::mock_scenario(model) || a != argv(role, root, model, &session) {
        return Err("ARGV");
    }
    let mut expected = std::collections::BTreeSet::new();
    for key in cli_adapter::ENV_ALLOW {
        expected.insert(key.to_ascii_uppercase());
    }
    for key in ["PATH", "TEMP", "TMP"] {
        expected.insert(key.into());
    }
    for (k, _) in std::env::vars_os() {
        let key = k.to_str().ok_or("ENV")?;
        if !expected.contains(&key.to_ascii_uppercase()) || cli_adapter::forbidden_env(key) {
            return Err("ENV");
        }
    }
    if role == RelayRole::Develop
        && std::fs::read(root.join("proposal.schema.json")).map_err(|_| "SCHEMA")?
            != proposal_schema().as_bytes()
    {
        return Err("SCHEMA");
    }
    let case = model.strip_prefix("mock:").ok_or("MODEL")?;
    if case == "no-stdin-exit" {
        return Ok(());
    }
    if case == "no-read" || case == "hang" {
        loop {
            std::thread::sleep(Duration::from_secs(60))
        }
    }
    if case == "stdout-bound" {
        let b = vec![b'x'; 1048577];
        std::io::stdout().write_all(&b).map_err(|_| "PIPE")?;
        return Ok(());
    }
    if case == "stderr-bound" {
        std::io::stderr()
            .write_all(&vec![b'x'; 1048577])
            .map_err(|_| "PIPE")?;
        return Ok(());
    }
    if ["duplex", "io"].contains(&case) {
        if role == RelayRole::Develop {
            emit(json!({"type":"thread.started","thread_id":"fixed-thread"}));
            emit(json!({"type":"turn.started"}));
        }
        for _ in 0..16 {
            if role == RelayRole::Develop {
                emit(
                    json!({"type":"item.updated","item":{"type":"reasoning","padding":"x".repeat(4096)}}),
                );
            } else {
                emit(json!({"type":"system","subtype":"status","padding":"x".repeat(4096)}));
            }
        }
    }
    let mut input = vec![];
    std::io::stdin()
        .take(131073)
        .read_to_end(&mut input)
        .map_err(|_| "INPUT")?;
    if input.len() > 131072 {
        return Err("INPUT");
    }
    let claim = if role == RelayRole::Develop {
        let v: Value = relay_json(&input, 131072)?;
        let base = v["base"].as_str().ok_or("INPUT")?;
        if !base.contains(RELAY_SIGNATURE) {
            return Err("INPUT");
        }
        json!({"schema":"nova.relay.proposal/1","target_path":"pilot/src/normalize_label.rs","old_text":"s.to_string()","new_text":"s.trim().to_lowercase()","rationale":"fixed mock proposal"})
    } else {
        let line = input.split(|b| *b == b'\n').next().ok_or("INPUT")?;
        let h: Value = relay_json(line, 65536)?;
        json!({"schema":"nova.relay.review/1","base_sha256":if case=="hash"{"0".repeat(64)}else{h["base_sha256"].as_str().ok_or("INPUT")?.into()},"candidate_sha256":h["candidate_sha256"],"verdict":if case=="blocked"{"BLOCKED"}else if case=="changes"{"CHANGES_REQUIRED"}else{"PASS"},"issues":if ["conflict","changes"].contains(&case){json!([{"id":"R-1","severity":"LOW","blocking":true,"location":"fixed","summary":"fixed"}])}else{json!([])},"unknowns":if case=="unknowns"{json!(["fixed unknown"])}else{json!([])}})
    };
    if case == "non-json" {
        println!("not JSON");
        return Ok(());
    }
    if case == "half" {
        print!("{{\"type\":");
        return Ok(());
    }
    if case == "unknown" {
        emit(json!({"type":"unregistered"}));
        return Ok(());
    }
    if role == RelayRole::Develop {
        if !["duplex", "io"].contains(&case) {
            emit(json!({"type":"thread.started","thread_id":"fixed-thread"}));
            emit(json!({"type":"turn.started"}));
        }
        if case == "tool" {
            emit(
                json!({"type":"item.started","item":{"type":"command_execution","command":"QUARANTINED_TOOL_SENTINEL"}}),
            );
            loop {
                std::thread::sleep(Duration::from_secs(60))
            }
        }
        if case == "descendant" {
            let mut child = Command::new(std::env::current_exe().map_err(|_| "SELF")?)
                .arg("--descendant")
                .stdin(Stdio::null())
                .spawn()
                .map_err(|_| "CHILD")?;
            emit(
                json!({"type":"item.completed","item":{"type":"reasoning","child_pid":child.id(),"child_alive":child.try_wait().map_err(|_|"CHILD_QUERY")?.is_none()}}),
            );
        }
        emit(
            json!({"type":"item.completed","item":{"type":"agent_message","text":claim.to_string()}}),
        );
        if case != "missing" {
            emit(
                json!({"type":if case=="turn-failed"{"turn.failed"}else if case=="error"{"error"}else{"turn.completed"},"usage":{"input_tokens":1,"output_tokens":1}}),
            );
        }
        if case == "duplicate" {
            emit(json!({"type":"turn.completed"}));
        }
    } else {
        emit(
            json!({"type":"system","subtype":"init","session_id":session,"tools":if case=="tools"{json!(["bad"])}else{json!([])},"mcp_servers":if case=="mcp"{json!(["bad"])}else{json!([])},"plugins":[]}),
        );
        if case == "hook" {
            emit(json!({"type":"system","subtype":"hook_started"}));
            return Ok(());
        }
        if case == "hook-top" {
            emit(json!({"type":"hook_started"}));
            return Ok(());
        }
        if case == "tool-use" {
            emit(
                json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use"}]}}),
            );
            return Ok(());
        }
        if case == "auth-retry" {
            emit(json!({"type":"system","subtype":"api_retry","error":"authentication_failed"}));
        }
        if case != "missing" {
            let mut v = json!({"type":"result","subtype":if case=="result-error"{"error_max_turns"}else{"success"},"is_error":case=="result-error","session_id":if case=="session"{"wrong-session"}else{&session},"permission_denials":if case=="denial"{json!(["fixed"])}else{json!([])},"structured_output":claim});
            if case == "no-structured" {
                v.as_object_mut().unwrap().remove("structured_output");
            }
            emit(v);
        }
    }
    if case == "after" {
        emit(json!({"type":"system","subtype":"status"}));
    }
    if case == "exit" {
        std::process::exit(7)
    }
    Ok(())
}
