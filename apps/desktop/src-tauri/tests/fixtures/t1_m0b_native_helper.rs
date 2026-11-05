#[cfg(not(debug_assertions))]
compile_error!("t1-api-eval is debug-only and must never ship");
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--env-contract"] => {
            // Presence only; no user paths, no secret types, no values or environment hashes.
            let username = std::env::var_os("OPENCODE_SERVER_USERNAME").is_some();
            let password = std::env::var_os("OPENCODE_SERVER_PASSWORD").is_some();
            let overlay = std::env::var_os("OPENCODE_CONFIG_CONTENT").is_some();
            println!("{{\"schema\":\"NB2_HELPER_V1\",\"git_secret_free\":{},\"opencode_credentials_present\":{}}}",!username&&!password&&!overlay,username&&password&&overlay);
        }
        ["--wait"] => loop {
            std::thread::park();
        },
        ["--child-wait"] => {
            let exe = std::env::current_exe().expect("own helper image");
            let mut child = std::process::Command::new(exe)
                .arg("--wait")
                .spawn()
                .expect("fixed self-child");
            let _ = child.wait();
        }
        _ => {
            eprintln!("fixed helper mode required");
            std::process::exit(77)
        }
    }
}
