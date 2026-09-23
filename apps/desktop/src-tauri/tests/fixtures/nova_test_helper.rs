//! Test-only helper binary for Nova's Windows process-spawn-race tests.
//!
//! Not part of Nova's user-facing application: `lib.rs`/`main.rs` never
//! invoke it, it is never referenced from any production code path, and
//! the Tauri bundler packages the app's own configured binary, not this
//! one. It exists as a separate `[[bin]]` target (auto-discovered by
//! Cargo from `src/bin/`) purely so `cargo test` has a small, real,
//! deterministic executable to launch instead of relying on PowerShell
//! startup timing — its path is available to tests at compile time via
//! `env!("CARGO_BIN_EXE_nova_test_helper")`.
//!
//! Every mode below only ever touches paths explicitly passed as
//! arguments (always a temp directory in the test suite) or its own
//! stdout — it never reads or writes a real user project.
//!
//! Modes (first argv):
//! - `echo-argv <args...>`: print every remaining argument on its own
//!   line, then exit 0. Used to prove argument-array fidelity end to end.
//! - `marker <path>`: write `<path>` immediately (the first thing this
//!   mode does, before anything else) to prove the process's own code
//!   had started running, then sleep. Used by the suspend-before-
//!   assignment ordering proof.
//! - `grandchild-race <pid-file>`: spawn a further child of *this*
//!   process (a grandchild relative to whatever spawned this helper) as
//!   the literal first statement in this branch — before any file I/O —
//!   then write the grandchild's PID to `<pid-file>` and sleep. Used by
//!   the immediate-grandchild race tests: there is no artificial delay
//!   for job assignment to "win" a race against.
//! - `sleep`: just sleep. Used as the grandchild's own body.
//! - `fake-cli <args...>`: a scriptable stand-in for a real `codex`/
//!   `claude` binary, used by the Phase 3 remediation CLI-adapter tests
//!   (neither real CLI is installed in every dev/CI environment, so the
//!   adapters' own argv/stdin/cwd/exit-code handling is verified against
//!   this real, separately-compiled `.exe` instead). Behavior is driven
//!   entirely by environment variables set by the spawning test, never by
//!   hardcoded knowledge of any real CLI's flags:
//!   - args exactly `["--version"]`: print `NOVA_FAKE_CLI_VERSION_TEXT`
//!     (default `fake-cli 0.0.0-test`), exit 0.
//!   - args exactly `["--help"]`: print `NOVA_FAKE_CLI_HELP_TEXT`, exit 0.
//!   - args end with `"--help"` but have more than one element (e.g.
//!     `exec --help`, `-p --help`): print `NOVA_FAKE_CLI_SUBHELP_TEXT`,
//!     exit 0.
//!   - otherwise (the "real" invocation): append one JSON line to the
//!     file at `NOVA_FAKE_CLI_LOG` recording `args`, the full
//!     UTF-8-lossy contents of stdin, and the process's current
//!     directory; then print the contents of the file at
//!     `NOVA_FAKE_CLI_RESPONSE_FILE` (if set) or the literal
//!     `NOVA_FAKE_CLI_RESPONSE_TEXT` (if set) to stdout, print
//!     `NOVA_FAKE_CLI_STDERR_TEXT` to stderr if set, and exit with the
//!     code from `NOVA_FAKE_CLI_EXIT_CODE` (default 0).

use std::env;
use std::fs;
use std::io::Read as _;
use std::process::Command;
use std::thread;
use std::time::Duration;

const SLEEP_SECONDS: u64 = 120;

fn main() {
    // `runtime::ai_adapter`'s tests drive this binary as a stand-in for a
    // real `codex`/`claude` CLI, whose real argv (`exec ...`, `-p ...`)
    // does not start with the `fake-cli` mode selector below. Rather than
    // an env var (process-global mutable state — a real race against every
    // *other* concurrently-running test in the same `cargo test` binary
    // that also spawns this helper in a different mode), those tests point
    // `CliConfig::binary_path` at a *renamed copy* of this exact exe (see
    // `ai_adapter::tests::fake_cli_config`) whose own file stem contains
    // `fake-cli` — a property of the specific file that was launched,
    // immune to any other thread's environment mutations.
    let is_fake_cli_identity = env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_lowercase())
        })
        .is_some_and(|stem| stem.contains("fake-cli") || stem.contains("fake_cli"));
    if is_fake_cli_identity {
        run_fake_cli(env::args().skip(1).collect());
        return;
    }

    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    match mode.as_str() {
        "echo-argv" => {
            for arg in args {
                println!("{arg}");
            }
        }
        "fake-cli" => run_fake_cli(args.collect()),
        "cat-stdin" => {
            let mut buffer = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut buffer);
            use std::io::Write as _;
            let _ = std::io::stdout().write_all(&buffer);
        }
        "marker" => {
            let marker_path = args.next().expect("marker path argument required");
            fs::write(&marker_path, b"started").expect("write marker file");
            thread::sleep(Duration::from_secs(SLEEP_SECONDS));
        }
        "grandchild-race" => {
            // First statement in this branch, before any file I/O: spawn
            // the grandchild immediately, so there is no window in which a
            // slow test harness could "help" job assignment win the race.
            let exe = env::current_exe().expect("current_exe");
            // Deliberately not `.wait()`-ed: the whole point of this test
            // fixture is for the grandchild to keep running independently
            // (in its own `sleep` mode) so the *test* can assert whether
            // Nova's Job Object containment actually terminates it.
            #[allow(clippy::zombie_processes)]
            let grandchild = Command::new(&exe)
                .arg("sleep")
                .spawn()
                .expect("spawn grandchild");
            let grandchild_pid = grandchild.id();
            // Only now, after the grandchild already exists, do anything
            // else. Written atomically (temp file, then rename into place)
            // so a concurrent reader can never observe a partially-written
            // file — matches Nova's own `atomic_write` discipline used
            // elsewhere in the real product for exactly the same reason.
            if let Some(pid_file) = args.next() {
                let tmp_path = format!("{pid_file}.tmp");
                fs::write(&tmp_path, grandchild_pid.to_string()).expect("write pid file tmp");
                fs::rename(&tmp_path, &pid_file).expect("rename pid file into place");
            }
            thread::sleep(Duration::from_secs(SLEEP_SECONDS));
        }
        "sleep" => {
            thread::sleep(Duration::from_secs(SLEEP_SECONDS));
        }
        other => {
            eprintln!("nova_test_helper: unknown mode {other:?}");
            std::process::exit(2);
        }
    }
}

fn run_fake_cli(args: Vec<String>) {
    let is_bare_help = args.len() == 1 && args[0] == "--help";
    let is_bare_version = args.len() == 1 && args[0] == "--version";
    let is_sub_help = args.len() > 1 && args.last().map(String::as_str) == Some("--help");

    if is_bare_version {
        let text = env::var("NOVA_FAKE_CLI_VERSION_TEXT")
            .unwrap_or_else(|_| "fake-cli 0.0.0-test".to_string());
        println!("{text}");
        return;
    }
    if is_bare_help {
        let text = env::var("NOVA_FAKE_CLI_HELP_TEXT").unwrap_or_default();
        println!("{text}");
        return;
    }
    if is_sub_help {
        let text = env::var("NOVA_FAKE_CLI_SUBHELP_TEXT").unwrap_or_default();
        println!("{text}");
        return;
    }

    // The "real" invocation: record argv + stdin + cwd for the test to
    // inspect, then answer with whatever the test scripted.
    let mut stdin_content = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin_content);
    let cwd = env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let record = format!(
        "{{\"args\":{},\"stdin\":{},\"cwd\":{}}}\n",
        json_string_array(&args),
        json_string(&stdin_content),
        json_string(&cwd),
    );
    if let Ok(log_path) = env::var("NOVA_FAKE_CLI_LOG") {
        use std::io::Write as _;
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = file.write_all(record.as_bytes());
        }
    }

    if let Ok(response_file) = env::var("NOVA_FAKE_CLI_RESPONSE_FILE") {
        if let Ok(contents) = fs::read_to_string(&response_file) {
            print!("{contents}");
        }
    } else if let Ok(text) = env::var("NOVA_FAKE_CLI_RESPONSE_TEXT") {
        print!("{text}");
    }
    if let Ok(stderr_text) = env::var("NOVA_FAKE_CLI_STDERR_TEXT") {
        eprint!("{stderr_text}");
    }
    let exit_code: i32 = env::var("NOVA_FAKE_CLI_EXIT_CODE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    std::process::exit(exit_code);
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_string_array(values: &[String]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_string(value));
    }
    out.push(']');
    out
}
