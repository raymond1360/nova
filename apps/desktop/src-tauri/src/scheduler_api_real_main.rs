#[cfg(not(debug_assertions))]
compile_error!("scheduler-validation is debug-only and must never ship");
use nova_lib::scheduler::{deepseek_transport as api, store::Guard};
use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};
fn line() -> Result<String, &'static str> {
    let mut b = String::new();
    io::stdin()
        .read_line(&mut b)
        .map_err(|_| "INPUT_REJECTED")?;
    if b.len() > 4096 {
        return Err("INPUT_BOUND");
    }
    Ok(b.trim_end_matches(['\r', '\n']).into())
}
#[cfg(windows)]
fn masked() -> Result<String, &'static str> {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE,
    };
    unsafe {
        let h = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(h, &mut mode) == 0 {
            return Err("CONSOLE_REQUIRED");
        }
        if SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT) == 0 {
            return Err("CONSOLE_REQUIRED");
        }
        let result = line();
        SetConsoleMode(h, mode);
        eprintln!();
        result
    }
}
#[cfg(not(windows))]
fn masked() -> Result<String, &'static str> {
    Err("CONSOLE_REQUIRED")
}
static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(windows)]
unsafe extern "system" fn cancel_handler(code: u32) -> i32 {
    if code == 0 || code == 1 {
        CANCEL.store(true, std::sync::atomic::Ordering::Release);
        1
    } else {
        0
    }
}
fn execute() -> Result<serde_json::Value, &'static str> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("status") if a.len() == 2 => api::status(Path::new(&a[1])),
        Some("run") if a.len() == 3 && a[2] == "--enable-real" => {
            #[cfg(windows)]
            unsafe {
                if windows_sys::Win32::System::Console::SetConsoleCtrlHandler(
                    Some(cancel_handler),
                    1,
                ) == 0
                {
                    return Err("CONSOLE_HANDLER_REJECTED");
                }
            }
            let result = api::run_cancel(Path::new(&a[1]), true, &CANCEL);
            #[cfg(windows)]
            unsafe {
                windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(cancel_handler), 0);
            }
            result
        }
        Some("configure-key") if a.len() == 1 => {
            if !io::stdin().is_terminal() {
                return Err("CONSOLE_REQUIRED");
            }
            eprint!("DeepSeek scheduler key (masked; independent slot): ");
            io::stderr().flush().map_err(|_| "INPUT_REJECTED")?;
            let mut key = masked()?;
            let r = api::configure_key(&key);
            unsafe { key.as_bytes_mut().fill(0) };
            r?;
            Ok(serde_json::json!({"credential_configured":true}))
        }
        Some("approve-init") if a.len() == 4 => {
            if !io::stdin().is_terminal() {
                return Err("CONSOLE_REQUIRED");
            }
            let mut f = Guard::open(Path::new(&a[3]), false)?;
            let raw = f.bytes(32768)?;
            let stdin = io::stdin();
            let stderr = io::stderr();
            api::approve_init_dialog(
                Path::new(&a[1]),
                Path::new(&a[2]),
                &raw,
                &mut stdin.lock(),
                &mut stderr.lock(),
            )
        }
        _ => Err("REAL_API_NOT_AUTHORIZED"),
    }
}
fn main() {
    match execute() {
        Ok(v) => println!("{v}"),
        Err(e) => {
            println!("{}", serde_json::json!({"error_code":e,"mode":"REAL_API"}));
            std::process::exit(2)
        }
    }
}
