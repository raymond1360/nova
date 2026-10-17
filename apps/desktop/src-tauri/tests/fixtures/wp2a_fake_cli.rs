//! Debug-only, no-network/no-spawn CLI fixture. Response is derived from actual stdin.
#[cfg(not(debug_assertions))]
compile_error!("validation fake CLI cannot ship");
use std::{
    fs,
    io::{Read, Write},
};
fn main() {
    let exe = std::env::current_exe().unwrap();
    let root = exe.parent().unwrap().parent().unwrap();
    assert!(
        root.join("ISOLATED-VALIDATION.json").is_file(),
        "missing explicit isolation marker"
    );
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mut record = serde_json::json!({"fixture":true,"exe":exe,"args":args,"cwd":std::env::current_dir().unwrap()});
    if args.iter().any(|a| a == "--version") {
        println!("NOVA_WP2A_FAKE_CLI_NO_NETWORK_NO_SPAWN_V1 0.1-validation");
    } else if args.iter().any(|a| a == "--help") {
        println!("exec --sandbox workspace-write read-only --full-auto -p --print --permission-mode --tools Read logged in ready");
    } else {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        let items: Vec<serde_json::Value> = input
            .split_once("===NOVA_REQUIREMENT_ITEMS_START===")
            .and_then(|(_, s)| s.split_once("===NOVA_REQUIREMENT_ITEMS_END==="))
            .and_then(|(s, _)| serde_json::from_str(s.trim()).ok())
            .expect("actual dispatched snapshot required");
        record["stdin"] = input.clone().into();
        let coverage: Vec<_> = items
            .iter()
            .map(|i| serde_json::json!({"item":i["item"],"status":"covered"}))
            .collect();
        let mut check = serde_json::json!({"requirement_coverage":coverage,"check_results":["FAKE CLI: inspected dispatched requirement identifiers"],"unfinished_items":[],"blocking_issues":[],"self_reported_status":"self_reported_complete"});
        if input.contains("SCENARIO:missing") {
            println!("FAKE CLI: self-check intentionally omitted");
        } else if input.contains("SCENARIO:invalid") {
            println!("===NOVA_SELF_CHECK_START===\n{{invalid\n===NOVA_SELF_CHECK_END===");
        } else {
            if input.contains("SCENARIO:incomplete") {
                check["self_reported_status"] = "self_reported_incomplete".into();
                check["unfinished_items"] = serde_json::json!(["FAKE: export unfinished"]);
            }
            println!("FAKE CLI, no real AI\n===NOVA_SELF_CHECK_START===\n{check}\n===NOVA_SELF_CHECK_END===");
        }
    }
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("fake-cli.jsonl"))
        .unwrap();
    writeln!(log, "{record}").unwrap();
}
