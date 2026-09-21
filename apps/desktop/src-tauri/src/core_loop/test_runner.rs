//! Phase 3 remediation (Blocker 3): independent target-project test
//! execution. Nova must never trust the developer AI's self-reported "tests
//! passed" — this module determines a test command safely (auto-detection
//! from a fixed list of known project markers, or a user-confirmed command),
//! validates it against a strict allowlist before ever running anything,
//! executes it through Phase 1's safe process boundary
//! (`runtime::process::execute_blocking` — argv array, no shell string,
//! timeout, cancellation, redaction, Job Object containment, all reused
//! unchanged), and produces [`TestRunOutcome`] for `core_loop` to record as
//! `test_evidence` independent of anything the AI claims.

use std::path::Path;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::runtime::process;
use crate::runtime::types::{ExecutionErrorKind, ExecutionOutcome, ExecutionRequest};

/// A test command Nova is willing to run — either safely auto-detected from
/// a known project-type marker, or explicitly confirmed by the user. Both
/// sources are legal per Blocker 3; an AI's raw suggestion is never one of
/// them (nothing in `core_loop` constructs a `DetectedCommand` from AI
/// output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl DetectedCommand {
    pub fn display(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

/// Read-only auto-detection: checks for a fixed list of well-known project
/// markers in `project_dir`'s top level only (never executes anything,
/// never reads AI output). Returns `None` when nothing recognizable is
/// found — the caller must then either ask the user to confirm a command or
/// record `no_tests_applicable`, never invent one.
pub fn detect_test_command(project_dir: &Path) -> Option<DetectedCommand> {
    if project_dir.join("Cargo.toml").is_file() {
        return Some(DetectedCommand {
            program: "cargo".to_string(),
            args: vec!["test".to_string()],
        });
    }
    if project_dir.join("package.json").is_file() {
        let program = if project_dir.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if project_dir.join("yarn.lock").is_file() {
            "yarn"
        } else {
            "npm"
        };
        return Some(DetectedCommand {
            program: program.to_string(),
            args: vec!["test".to_string()],
        });
    }
    if project_dir.join("pytest.ini").is_file()
        || project_dir.join("pyproject.toml").is_file()
        || project_dir.join("setup.cfg").is_file()
    {
        return Some(DetectedCommand {
            program: "python".to_string(),
            args: vec!["-m".to_string(), "pytest".to_string()],
        });
    }
    None
}

/// Strict allowlist: the *only* `(program, args)` shapes Nova will ever
/// execute as a "test run", whether the command came from auto-detection or
/// a user-typed confirmation. Never bypassed, never extended by AI output —
/// this is the hard boundary that stops an AI-suggested arbitrary command
/// from ever being executed as if it were a trusted test invocation.
pub fn is_allowlisted(program: &str, args: &[String]) -> bool {
    let first = args.first().map(String::as_str);
    let second = args.get(1).map(String::as_str);
    match program {
        "cargo" => first == Some("test"),
        "npm" | "pnpm" | "yarn" => first == Some("test") || first == Some("run"),
        "python" | "python3" => first == Some("-m") && second == Some("pytest"),
        "pytest" => true,
        _ => false,
    }
}

/// Codex re-review remediation (Major 3): Nova's own fact-verification must
/// distinguish "tests passed" from "the project actually builds" — a
/// project whose tests pass but whose build fails must never be reported as
/// fully verified. Minimal, safe scope only: `cargo build` (always
/// applicable for a Cargo project — Rust code without a successful `cargo
/// build` cannot meaningfully be said to work at all), or `npm run build` /
/// `pnpm run build` / `yarn build`, but *only* when `package.json` itself
/// declares a `build` script — checked by parsing real JSON, never by
/// string-searching the file (a project with no `build` script has,
/// mechanically and verifiably, no build step for Nova to run; this is a
/// hard fact from the manifest itself, not a heuristic that could miss a
/// real framework the way test-command auto-detection can, so no user
/// confirmation gate is needed the way Blocker 3's `no_tests_applicable`
/// one is).
pub enum BuildDetection {
    Command(DetectedCommand),
    /// No recognizable build command for this project — carries the reason
    /// so it can be recorded as `no_build_applicable` evidence.
    NotApplicable(&'static str),
}

pub fn detect_build_command(project_dir: &Path) -> BuildDetection {
    if project_dir.join("Cargo.toml").is_file() {
        return BuildDetection::Command(DetectedCommand {
            program: "cargo".to_string(),
            args: vec!["build".to_string()],
        });
    }
    if project_dir.join("package.json").is_file() {
        let has_build_script = std::fs::read_to_string(project_dir.join("package.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| {
                value
                    .get("scripts")
                    .and_then(|scripts| scripts.get("build"))
                    .and_then(|build| build.as_str())
                    .map(|s| !s.trim().is_empty())
            })
            .unwrap_or(false);
        if !has_build_script {
            return BuildDetection::NotApplicable(
                "npm/pnpm/yarn 项目的 package.json 未声明 build 脚本",
            );
        }
        let program = if project_dir.join("pnpm-lock.yaml").is_file() {
            "pnpm"
        } else if project_dir.join("yarn.lock").is_file() {
            "yarn"
        } else {
            "npm"
        };
        let args = if program == "yarn" {
            vec!["build".to_string()]
        } else {
            vec!["run".to_string(), "build".to_string()]
        };
        return BuildDetection::Command(DetectedCommand {
            program: program.to_string(),
            args,
        });
    }
    BuildDetection::NotApplicable("未识别到可执行的构建命令（无 Cargo.toml 或 package.json）")
}

/// Strict allowlist for build commands — mirrors [`is_allowlisted`]'s
/// discipline, a hard boundary independent of (and never widened by)
/// anything an AI suggests.
pub fn is_build_allowlisted(program: &str, args: &[String]) -> bool {
    let first = args.first().map(String::as_str);
    let second = args.get(1).map(String::as_str);
    match program {
        "cargo" => first == Some("build"),
        "npm" | "pnpm" => first == Some("run") && second == Some("build"),
        "yarn" => first == Some("build"),
        _ => false,
    }
}

pub struct TestRunOutcome {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub passed: Option<i64>,
    pub failed: Option<i64>,
    pub skipped: Option<i64>,
    pub timed_out: bool,
    pub cancelled: bool,
    /// True only for a real, clean-exit (`exit_code == Some(0)`) run.
    pub process_succeeded: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `program args` in `project_dir` through Phase 1's safe process
/// boundary and classifies the outcome. Callers must have already checked
/// [`is_allowlisted`] — this function does not re-check it, so it must never
/// be reachable with an unvalidated command.
pub fn run_tests(
    program: &str,
    args: &[String],
    project_dir: &Path,
    timeout: Duration,
    cancel: CancellationToken,
) -> TestRunOutcome {
    let request = ExecutionRequest::new(
        program.to_string(),
        args.to_vec(),
        project_dir.to_path_buf(),
        Some(project_dir.to_path_buf()),
    )
    .with_timeout(timeout);
    let result = process::execute_blocking(request, cancel);

    let timed_out = matches!(
        result.outcome,
        ExecutionOutcome::Failed {
            kind: ExecutionErrorKind::TimedOut,
            ..
        }
    );
    let cancelled = matches!(
        result.outcome,
        ExecutionOutcome::Failed {
            kind: ExecutionErrorKind::Cancelled,
            ..
        }
    );
    let process_succeeded = matches!(result.outcome, ExecutionOutcome::Success);
    let combined = format!("{}\n{}", result.stdout, result.stderr);
    let (passed, failed, skipped) = parse_counts(&combined);

    TestRunOutcome {
        program: result.program,
        args: result.args,
        cwd: result.cwd,
        exit_code: result.exit_code,
        passed,
        failed,
        skipped,
        timed_out,
        cancelled,
        process_succeeded,
        stdout: result.stdout,
        stderr: result.stderr,
    }
}

/// Best-effort, framework-agnostic pass/fail/skip count extraction. Looks
/// for the last `<number> <label>` occurrence for each of `passed`/`failed`/
/// (`skipped` or `ignored`) — a single generic pattern that happens to match
/// `cargo test`'s `"3 passed; 0 failed; 1 ignored"`, pytest's `"5 passed, 2
/// failed, 1 skipped in 1.23s"`, and `npm test`/Jest's `"Tests: 1 failed, 11
/// passed, 12 total"` without needing a separate parser per framework. Never
/// guessed when no matching pattern exists — all three come back `None`
/// rather than `Some(0)`, so "no parseable summary" is never confused with
/// "zero tests ran".
fn parse_counts(text: &str) -> (Option<i64>, Option<i64>, Option<i64>) {
    let passed = extract_count_before(text, "passed");
    let failed = extract_count_before(text, "failed");
    let skipped =
        extract_count_before(text, "skipped").or_else(|| extract_count_before(text, "ignored"));
    (passed, failed, skipped)
}

/// Scans every occurrence of `label` in `text`, from the last to the first,
/// and returns the integer immediately preceding the first occurrence that
/// actually has one (skipping whitespace). Trying only the very last
/// occurrence is not safe: `stdout`+`stderr` are concatenated, and stderr
/// commonly carries its own label-shaped-but-countless text (e.g. cargo's
/// `"error: test failed, to rerun pass ..."`, which has no digit before
/// `"failed"` at all) *after* the real `"N failed"` summary line already
/// present in stdout — a naive "last match wins" would silently return
/// `None`/wrong data for a summary that genuinely exists earlier.
fn extract_count_before(text: &str, label: &str) -> Option<i64> {
    let mut search_end = text.len();
    while let Some(idx) = text[..search_end].rfind(label) {
        let before = &text[..idx];
        let trimmed = before.trim_end();
        let digits_start = trimmed
            .rfind(|c: char| !c.is_ascii_digit())
            .map(|i| i + 1)
            .unwrap_or(0);
        let digits = &trimmed[digits_start..];
        if !digits.is_empty() {
            if let Ok(value) = digits.parse::<i64>() {
                return Some(value);
            }
        }
        search_end = idx;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-test-runner-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn detects_cargo_project() {
        let dir = temp_dir("cargo");
        fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let detected = detect_test_command(&dir).unwrap();
        assert_eq!(detected.program, "cargo");
        assert_eq!(detected.args, vec!["test".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_npm_project_with_no_lockfile_as_npm() {
        let dir = temp_dir("npm-plain");
        fs::write(dir.join("package.json"), "{}").unwrap();
        let detected = detect_test_command(&dir).unwrap();
        assert_eq!(detected.program, "npm");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_pnpm_project_via_lockfile() {
        let dir = temp_dir("pnpm");
        fs::write(dir.join("package.json"), "{}").unwrap();
        fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        let detected = detect_test_command(&dir).unwrap();
        assert_eq!(detected.program, "pnpm");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_yarn_project_via_lockfile() {
        let dir = temp_dir("yarn");
        fs::write(dir.join("package.json"), "{}").unwrap();
        fs::write(dir.join("yarn.lock"), "").unwrap();
        let detected = detect_test_command(&dir).unwrap();
        assert_eq!(detected.program, "yarn");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_python_project_via_pyproject_toml() {
        let dir = temp_dir("pytest");
        fs::write(dir.join("pyproject.toml"), "[tool.pytest.ini_options]").unwrap();
        let detected = detect_test_command(&dir).unwrap();
        assert_eq!(detected.program, "python");
        assert_eq!(detected.args, vec!["-m".to_string(), "pytest".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_nothing_for_an_unrecognized_project() {
        let dir = temp_dir("unknown");
        fs::write(dir.join("README.md"), "hello").unwrap();
        assert!(detect_test_command(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Codex re-review remediation (Major 3): build detection ---------

    #[test]
    fn detects_cargo_build_for_a_cargo_project() {
        let dir = temp_dir("build-cargo");
        fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        match detect_build_command(&dir) {
            BuildDetection::Command(command) => {
                assert_eq!(command.program, "cargo");
                assert_eq!(command.args, vec!["build".to_string()]);
            }
            BuildDetection::NotApplicable(reason) => {
                panic!("expected a detected cargo build command, got: {reason}")
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_npm_run_build_when_package_json_declares_a_build_script() {
        let dir = temp_dir("build-npm-with-script");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","scripts":{"build":"vite build","test":"vitest run"}}"#,
        )
        .unwrap();
        match detect_build_command(&dir) {
            BuildDetection::Command(command) => {
                assert_eq!(command.program, "npm");
                assert_eq!(command.args, vec!["run".to_string(), "build".to_string()]);
            }
            BuildDetection::NotApplicable(reason) => {
                panic!("expected a detected npm build command, got: {reason}")
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_no_build_applicable_when_package_json_has_no_build_script() {
        let dir = temp_dir("build-npm-without-script");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","scripts":{"test":"vitest run"}}"#,
        )
        .unwrap();
        match detect_build_command(&dir) {
            BuildDetection::Command(command) => {
                panic!("expected no_build_applicable, got a command: {command:?}")
            }
            BuildDetection::NotApplicable(_) => {}
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_pnpm_run_build_via_lockfile_when_build_script_present() {
        let dir = temp_dir("build-pnpm");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","scripts":{"build":"tsc"}}"#,
        )
        .unwrap();
        fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        match detect_build_command(&dir) {
            BuildDetection::Command(command) => {
                assert_eq!(command.program, "pnpm");
                assert_eq!(command.args, vec!["run".to_string(), "build".to_string()]);
            }
            BuildDetection::NotApplicable(reason) => {
                panic!("expected a detected pnpm build command, got: {reason}")
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_yarn_build_via_lockfile_when_build_script_present() {
        let dir = temp_dir("build-yarn");
        fs::write(
            dir.join("package.json"),
            r#"{"name":"x","scripts":{"build":"webpack"}}"#,
        )
        .unwrap();
        fs::write(dir.join("yarn.lock"), "").unwrap();
        match detect_build_command(&dir) {
            BuildDetection::Command(command) => {
                assert_eq!(command.program, "yarn");
                assert_eq!(command.args, vec!["build".to_string()]);
            }
            BuildDetection::NotApplicable(reason) => {
                panic!("expected a detected yarn build command, got: {reason}")
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_no_build_applicable_for_an_unrecognized_project() {
        let dir = temp_dir("build-unknown");
        fs::write(dir.join("README.md"), "hello").unwrap();
        match detect_build_command(&dir) {
            BuildDetection::Command(command) => {
                panic!("expected no_build_applicable, got a command: {command:?}")
            }
            BuildDetection::NotApplicable(_) => {}
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_allowlist_accepts_only_the_documented_shapes() {
        assert!(is_build_allowlisted("cargo", &["build".to_string()]));
        assert!(is_build_allowlisted(
            "npm",
            &["run".to_string(), "build".to_string()]
        ));
        assert!(is_build_allowlisted(
            "pnpm",
            &["run".to_string(), "build".to_string()]
        ));
        assert!(is_build_allowlisted("yarn", &["build".to_string()]));
    }

    #[test]
    fn build_allowlist_rejects_an_ai_suggested_arbitrary_command() {
        assert!(!is_build_allowlisted(
            "bash",
            &["-c".to_string(), "rm -rf /".to_string()]
        ));
        assert!(!is_build_allowlisted("cargo", &["publish".to_string()]));
        assert!(!is_build_allowlisted("npm", &["publish".to_string()]));
        assert!(!is_build_allowlisted("npm", &["test".to_string()]));
    }

    #[test]
    fn allowlist_accepts_only_the_documented_shapes() {
        assert!(is_allowlisted("cargo", &["test".to_string()]));
        assert!(is_allowlisted("npm", &["test".to_string()]));
        assert!(is_allowlisted(
            "npm",
            &["run".to_string(), "test".to_string()]
        ));
        assert!(is_allowlisted("pnpm", &["test".to_string()]));
        assert!(is_allowlisted("yarn", &["test".to_string()]));
        assert!(is_allowlisted(
            "python",
            &["-m".to_string(), "pytest".to_string()]
        ));
        assert!(is_allowlisted("pytest", &[]));
    }

    #[test]
    fn allowlist_rejects_an_ai_suggested_arbitrary_command() {
        assert!(!is_allowlisted(
            "bash",
            &["-c".to_string(), "rm -rf /".to_string()]
        ));
        assert!(!is_allowlisted("curl", &["evil.example.com".to_string()]));
        assert!(!is_allowlisted("cargo", &["publish".to_string()]));
        assert!(!is_allowlisted("npm", &["publish".to_string()]));
        assert!(!is_allowlisted(
            "python",
            &[
                "-c".to_string(),
                "import os; os.system('rm -rf /')".to_string()
            ]
        ));
    }

    #[test]
    fn parses_cargo_test_output() {
        let text = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s";
        let (passed, failed, skipped) = parse_counts(text);
        assert_eq!(passed, Some(3));
        assert_eq!(failed, Some(0));
        assert_eq!(skipped, Some(1));
    }

    #[test]
    fn parses_pytest_output() {
        let text = "===== 5 passed, 2 failed, 1 skipped in 1.23s =====";
        let (passed, failed, skipped) = parse_counts(text);
        assert_eq!(passed, Some(5));
        assert_eq!(failed, Some(2));
        assert_eq!(skipped, Some(1));
    }

    #[test]
    fn parses_jest_style_output() {
        let text = "Tests:       1 failed, 11 passed, 12 total";
        let (passed, failed, skipped) = parse_counts(text);
        assert_eq!(passed, Some(11));
        assert_eq!(failed, Some(1));
        assert_eq!(skipped, None);
    }

    #[test]
    fn returns_none_for_unparseable_output_rather_than_guessing_zero() {
        let text = "some unrelated build log output with no summary line";
        assert_eq!(parse_counts(text), (None, None, None));
    }

    #[test]
    fn a_trailing_countless_stderr_failed_line_does_not_shadow_stdouts_real_summary_count() {
        // Regression test: `run_tests` concatenates stdout + stderr. Real
        // `cargo test` failure output puts the genuine "N passed; M failed"
        // summary in stdout, then a separate, countless
        // "error: test failed, to rerun pass `--lib`" line in stderr —
        // which comes *later* in the concatenation and must never shadow
        // the real count that appears earlier.
        let stdout = "running 1 test\ntest x ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        let stderr = "error: test failed, to rerun pass `--lib`\n";
        let combined = format!("{stdout}\n{stderr}");
        let (passed, failed, _skipped) = parse_counts(&combined);
        assert_eq!(passed, Some(0));
        assert_eq!(failed, Some(1));
    }

    #[test]
    fn real_cargo_test_run_against_this_crate_produces_a_real_pass_count() {
        // A genuine end-to-end proof: run `cargo test --lib app_paths::tests::accepts_ordinary_uuid_like_components`
        // against this actual crate and confirm Nova's own parser reads a
        // real, non-zero passed count from the real output.
        //
        // `--target-dir` points this nested build at its own directory
        // rather than the crate's real `target/` — without it, this inner
        // `cargo test` tries to relink the exact `nova_lib-*.exe` the
        // *outer* `cargo test` run is currently executing from, which
        // fails on Windows with `LNK1104: unable to open file` (the file
        // is locked while running) whenever this test happens to run
        // concurrently with others in the same suite. Deliberately a
        // *fixed* path under the OS temp directory (not a fresh
        // `uuid`-suffixed one per run, unlike every other fixture in this
        // file) so it doubles as a persistent build cache across repeated
        // test-suite runs — a fresh `--target-dir` would force a full,
        // multi-minute rebuild of this crate's entire dependency tree
        // (`tauri`, `reqwest`, `rustls`, ...) on every single run, which
        // both blows well past this test's own timeout and would make
        // repeated full-suite verification runs impractically slow.
        // Intentionally never deleted for the same reason.
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let cached_target_dir = std::env::temp_dir().join("nova-self-referential-cargo-test-cache");
        let outcome = run_tests(
            "cargo",
            &[
                "test".to_string(),
                "--lib".to_string(),
                "app_paths::tests::accepts_ordinary_uuid_like_components".to_string(),
                "--target-dir".to_string(),
                cached_target_dir.to_string_lossy().into_owned(),
            ],
            &manifest_dir,
            Duration::from_secs(300),
            CancellationToken::new(),
        );
        assert!(outcome.process_succeeded, "stderr: {}", outcome.stderr);
        assert_eq!(outcome.passed, Some(1));
        assert_eq!(outcome.failed, Some(0));
    }

    #[test]
    fn a_timed_out_run_is_classified_as_timed_out_not_a_parse_failure() {
        let dir = temp_dir("timeout-run");
        let outcome = run_tests(
            "ping",
            &["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()],
            &dir,
            Duration::from_millis(300),
            CancellationToken::new(),
        );
        assert!(outcome.timed_out);
        assert!(!outcome.process_succeeded);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_run_is_classified_as_cancelled() {
        let dir = temp_dir("cancel-run");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = run_tests(
            "ping",
            &["-n".to_string(), "5".to_string(), "127.0.0.1".to_string()],
            &dir,
            Duration::from_secs(30),
            cancel,
        );
        assert!(outcome.cancelled);
        assert!(!outcome.process_succeeded);
        let _ = fs::remove_dir_all(&dir);
    }
}
