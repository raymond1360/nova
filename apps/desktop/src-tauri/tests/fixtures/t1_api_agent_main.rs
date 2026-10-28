#[cfg(not(debug_assertions))]
compile_error!("t1-api-eval is debug-only and must never ship");
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args != ["--mock-self-test"] {
        eprintln!("NOT AUTHORIZED: only fixed Nova-side SYNTHETIC mock entry");
        std::process::exit(77);
    }
    match nova_lib::t1_api_eval::mock_run() {
        Ok(v) => println!("{v}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
