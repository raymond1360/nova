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

use std::env;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

const SLEEP_SECONDS: u64 = 120;

fn main() {
    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    match mode.as_str() {
        "echo-argv" => {
            for arg in args {
                println!("{arg}");
            }
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
            // else.
            if let Some(pid_file) = args.next() {
                fs::write(&pid_file, grandchild_pid.to_string()).expect("write pid file");
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
