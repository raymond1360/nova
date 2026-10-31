#[cfg(not(debug_assertions))]
compile_error!("t1-api-eval is debug-only and must never ship");

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--mock-self-test"] {
        match nova_lib::t1_api_eval::opencode_probe::mock_self_test() {
            Ok(v) => {println!("{v}"); return;},
            Err(e) => {eprintln!("{e}"); std::process::exit(1);}
        }
    }
    if args != ["--offline-probe-once"] {
        eprintln!("Only the fixed --mock-self-test / --offline-probe-once entries is recognized");
        std::process::exit(77);
    }
    // No resources or facility receipts were authorized by this development pack.
    // This gate performs no process, Job, Git, observer or fixture operation.
    let policy = include_str!("../../../../../tools/t1-api-eval/m0b/resource-policy.json");
    match nova_lib::t1_api_eval::opencode_probe::resource_admission(policy) {
        Ok(()) => {
            eprintln!("BLOCKED: native execution backend requires separate V-1..V-6 authorization");
            std::process::exit(77);
        }
        Err(e) => {
            eprintln!("BLOCKED: {e}");
            std::process::exit(77);
        }
    }
}
