//! Deliberately synthetic CLI. Never invokes Codex, tools, a provider or a session store.
#[cfg(not(debug_assertions))]
compile_error!("scheduler-validation is debug-only and must never ship");
use std::io::{Read, Write};
fn main() {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if a.first().map(String::as_str) != Some("--nova-self2-synthetic-only") {
        std::process::exit(2);
    }
    if a.iter().any(|a| a == "--fixed-check") {
        println!("SYNTHETIC fixed verification executable, exit 0");
        return;
    }
    let id = a
        .windows(2)
        .find(|w| w[0] == "resume")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut b = vec![];
    if std::io::stdin()
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut b)
        .is_err()
    {
        std::process::exit(2);
    }
    match nova_lib::scheduler::self_codex::fixture_wire(&b, &id) {
        Ok(out) => {
            std::io::stdout().write_all(&out).unwrap();
        }
        Err(_) => std::process::exit(2),
    }
}
