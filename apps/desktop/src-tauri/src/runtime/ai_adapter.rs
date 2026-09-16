//! Phase 3 (Open Local Project Dual-AI Core Loop): the pluggable interface
//! `storage::core_loop` uses to invoke a developer AI and an independent
//! reviewer AI, both through Phase 1's safe process-execution boundary
//! (`runtime::process`) — argument-array execution, no shell string,
//! timeout, cancellation, and redaction all reused unchanged.
//!
//! [`RealCliAdapter`] is the production implementation, shelling out to the
//! real `codex`/`claude` binaries. [`MockAdapter`] is the test-only
//! implementation used by every automated test in this codebase (per the
//! Phase 3 plan's own local-test list: "Mock Codex success/failure",
//! "Mock Claude review APPROVE/CHANGES_REQUIRED/INSUFFICIENT_EVIDENCE") so
//! the test suite never depends on an actually-installed, logged-in CLI.

use std::path::PathBuf;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::process;
use super::types::{ExecutionRequest, ExecutionResult, ToolKind};

/// Binary name for a `ToolKind`. Mirrors `runtime::detect`'s private
/// `program_for` mapping (kept private there); duplicated here as a
/// minimal 3-line match rather than widening that already-accepted Phase 1
/// file's visibility for a single caller.
pub fn binary_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Git => "git",
        ToolKind::CodexCli => "codex",
        ToolKind::ClaudeCli => "claude",
    }
}

/// Storage-layer string form of a `ToolKind`, for the `tasks.ai_kind` /
/// `task_reviews.reviewer_ai_kind` TEXT columns — matches the
/// `#[serde(rename_all = "snake_case")]` spelling `ToolKind` already uses.
pub fn ai_kind_to_str(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Git => "git",
        ToolKind::CodexCli => "codex_cli",
        ToolKind::ClaudeCli => "claude_cli",
    }
}

pub fn ai_kind_from_str(value: &str) -> Option<ToolKind> {
    match value {
        "codex_cli" => Some(ToolKind::CodexCli),
        "claude_cli" => Some(ToolKind::ClaudeCli),
        _ => None,
    }
}

/// Role swap: Nova only ever pairs the two supported CLIs against each
/// other — the reviewer is always "the other one" from the developer.
pub fn the_other(kind: ToolKind) -> ToolKind {
    match kind {
        ToolKind::CodexCli => ToolKind::ClaudeCli,
        ToolKind::ClaudeCli => ToolKind::CodexCli,
        ToolKind::Git => ToolKind::Git,
    }
}

pub struct DevelopmentRequest {
    pub ai_kind: ToolKind,
    pub project_dir: PathBuf,
    /// Path to the Markdown task package Nova wrote (requirement, scope,
    /// acceptance criteria, prior review findings for a fix pass) — see
    /// `storage::core_loop::write_task_package`.
    pub package_path: PathBuf,
    pub timeout: Duration,
}

pub struct ReviewRequest {
    pub ai_kind: ToolKind,
    pub project_dir: PathBuf,
    pub package_path: PathBuf,
    pub timeout: Duration,
}

/// Structured review outcome — the only three verdicts the product spec
/// allows a real review round to produce. `storage::core_loop` layers a
/// fourth, Nova-internal state (`integrity_violation`) on top of this when
/// its own technical read-only check fails; that is deliberately not a
/// variant here, since an adapter must never claim to know that on its own
/// (only the orchestrator, which controls the git snapshot comparison, can
/// determine it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    ChangesRequired,
    InsufficientEvidence,
}

impl ReviewVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::ChangesRequired => "changes_required",
            Self::InsufficientEvidence => "insufficient_evidence",
        }
    }
}

/// The line a reviewer's output must end with for Nova to recognize its
/// verdict. This is a Nova-defined, documented convention embedded in every
/// review task package's instructions — not a guess at undocumented real
/// CLI behavior. See the Phase 3 report's Known Limitations for why this
/// convention, rather than a confirmed real-CLI structured-output flag, is
/// what `RealCliAdapter` currently relies on.
pub const VERDICT_MARKER: &str = "NOVA_REVIEW_VERDICT:";

/// Scans `text` for the last `NOVA_REVIEW_VERDICT: <value>` line and parses
/// it. Deliberately takes the *last* match (a reviewer's prose may mention
/// the marker name while explaining the convention) and is whitespace/case
/// tolerant on the value itself.
pub fn parse_review_verdict(text: &str) -> Option<ReviewVerdict> {
    text.lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix(VERDICT_MARKER))
        .map(str::trim)
        .and_then(|value| match value.to_ascii_uppercase().as_str() {
            "APPROVE" => Some(ReviewVerdict::Approve),
            "CHANGES_REQUIRED" => Some(ReviewVerdict::ChangesRequired),
            "INSUFFICIENT_EVIDENCE" => Some(ReviewVerdict::InsufficientEvidence),
            _ => None,
        })
}

/// Developer/reviewer AI invocation, abstracted so `storage::core_loop` can
/// be driven by either a real CLI or a deterministic test double. Methods
/// are synchronous (not `async fn`) so `dyn DevReviewAdapter` stays object
/// safe with no extra dependency (no `async-trait`) — callers invoke these
/// from inside `tokio::task::spawn_blocking`, the same pattern
/// `runtime::process::execute` itself uses internally.
pub trait DevReviewAdapter: Send + Sync {
    fn run_development(
        &self,
        request: &DevelopmentRequest,
        cancel: CancellationToken,
    ) -> ExecutionResult;
    fn run_review(&self, request: &ReviewRequest, cancel: CancellationToken) -> ExecutionResult;
}

/// Production adapter: shells out to the real `codex`/`claude` binary
/// through Phase 1's safe process-execution boundary.
///
/// The exact invocation (`exec <package-path>`) is a best-effort, isolated
/// convention — this environment does not have a confirmed, logged-in
/// installation of either real CLI to validate the actual flag/subcommand
/// syntax against, so this is intentionally the *only* place that syntax
/// lives, and is called out as a known limitation in the Phase 3 report
/// rather than left undocumented.
pub struct RealCliAdapter;

impl RealCliAdapter {
    fn invoke(
        program: &str,
        package_path: &std::path::Path,
        project_dir: &std::path::Path,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        let request = ExecutionRequest::new(
            program.to_string(),
            vec![
                "exec".to_string(),
                package_path.to_string_lossy().into_owned(),
            ],
            project_dir.to_path_buf(),
            Some(project_dir.to_path_buf()),
        )
        .with_timeout(timeout);
        process::execute_blocking(request, cancel)
    }
}

impl DevReviewAdapter for RealCliAdapter {
    fn run_development(
        &self,
        request: &DevelopmentRequest,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        Self::invoke(
            binary_name(request.ai_kind),
            &request.package_path,
            &request.project_dir,
            request.timeout,
            cancel,
        )
    }

    fn run_review(&self, request: &ReviewRequest, cancel: CancellationToken) -> ExecutionResult {
        Self::invoke(
            binary_name(request.ai_kind),
            &request.package_path,
            &request.project_dir,
            request.timeout,
            cancel,
        )
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    type ReviewSideEffect = Box<dyn Fn(&ReviewRequest) + Send>;

    /// Deterministic, no-process-execution test double. Every call is
    /// recorded (for assertions on what the orchestrator actually
    /// requested) and answered from a caller-supplied script, so tests
    /// never depend on a real Codex/Claude installation.
    pub struct MockAdapter {
        development_script: Mutex<Vec<MockOutcome>>,
        review_script: Mutex<Vec<ReviewVerdict>>,
        development_calls: AtomicUsize,
        review_calls: AtomicUsize,
        /// If set, `run_review` also performs this side effect (used to
        /// simulate a reviewer that violates read-only enforcement by
        /// writing to the project directory).
        pub review_side_effect: Mutex<Option<ReviewSideEffect>>,
        /// If set, prepended to the synthesized review stdout (before the
        /// verdict marker line) — lets a test control exactly what "review
        /// findings" text reaches the orchestrator, e.g. to prove it gets
        /// redacted before being persisted.
        pub review_findings_text: Mutex<Option<String>>,
    }

    #[derive(Clone)]
    pub enum MockOutcome {
        Success,
        Failure,
    }

    impl MockAdapter {
        pub fn new(
            development_script: Vec<MockOutcome>,
            review_script: Vec<ReviewVerdict>,
        ) -> Self {
            Self {
                development_script: Mutex::new(development_script),
                review_script: Mutex::new(review_script),
                development_calls: AtomicUsize::new(0),
                review_calls: AtomicUsize::new(0),
                review_side_effect: Mutex::new(None),
                review_findings_text: Mutex::new(None),
            }
        }

        pub fn development_call_count(&self) -> usize {
            self.development_calls.load(Ordering::SeqCst)
        }

        pub fn review_call_count(&self) -> usize {
            self.review_calls.load(Ordering::SeqCst)
        }

        fn synthetic_result(outcome_is_success: bool, extra_stdout: &str) -> ExecutionResult {
            use super::super::types::{ExecutionErrorKind, ExecutionOutcome};
            let now = chrono::Utc::now();
            ExecutionResult {
                program: "mock".to_string(),
                args: vec![],
                cwd: String::new(),
                started_at: now,
                ended_at: now,
                duration_ms: 0,
                exit_code: Some(if outcome_is_success { 0 } else { 1 }),
                stdout: extra_stdout.to_string(),
                stderr: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                outcome: if outcome_is_success {
                    ExecutionOutcome::Success
                } else {
                    ExecutionOutcome::Failed {
                        kind: ExecutionErrorKind::NonZeroExit,
                        detail: "mock failure".to_string(),
                    }
                },
            }
        }
    }

    impl DevReviewAdapter for MockAdapter {
        fn run_development(
            &self,
            _request: &DevelopmentRequest,
            _cancel: CancellationToken,
        ) -> ExecutionResult {
            self.development_calls.fetch_add(1, Ordering::SeqCst);
            let mut script = self.development_script.lock().unwrap();
            let outcome = if script.is_empty() {
                MockOutcome::Success
            } else {
                script.remove(0)
            };
            Self::synthetic_result(matches!(outcome, MockOutcome::Success), "")
        }

        fn run_review(
            &self,
            request: &ReviewRequest,
            _cancel: CancellationToken,
        ) -> ExecutionResult {
            self.review_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(effect) = self.review_side_effect.lock().unwrap().as_ref() {
                effect(request);
            }
            let mut script = self.review_script.lock().unwrap();
            let verdict = if script.is_empty() {
                ReviewVerdict::Approve
            } else {
                script.remove(0)
            };
            let findings = self
                .review_findings_text
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "Review complete.".to_string());
            let stdout = format!(
                "{findings}\n{VERDICT_MARKER} {}",
                verdict.as_str().to_ascii_uppercase()
            );
            Self::synthetic_result(true, &stdout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_trailing_approve_marker() {
        let text = "Looks good, requirements met.\nNOVA_REVIEW_VERDICT: APPROVE";
        assert_eq!(parse_review_verdict(text), Some(ReviewVerdict::Approve));
    }

    #[test]
    fn parses_case_and_whitespace_tolerantly() {
        let text = "notes\nNOVA_REVIEW_VERDICT:   changes_required   ";
        assert_eq!(
            parse_review_verdict(text),
            Some(ReviewVerdict::ChangesRequired)
        );
    }

    #[test]
    fn returns_none_when_no_marker_present() {
        assert_eq!(parse_review_verdict("just some prose with no marker"), None);
    }

    #[test]
    fn returns_none_for_an_unrecognized_value() {
        assert_eq!(parse_review_verdict("NOVA_REVIEW_VERDICT: MAYBE"), None);
    }

    #[test]
    fn uses_the_last_marker_line_when_several_are_present() {
        let text = "Explaining the convention: NOVA_REVIEW_VERDICT: <value>\nNOVA_REVIEW_VERDICT: INSUFFICIENT_EVIDENCE";
        assert_eq!(
            parse_review_verdict(text),
            Some(ReviewVerdict::InsufficientEvidence)
        );
    }

    #[test]
    fn role_swap_always_pairs_the_two_supported_clis() {
        assert_eq!(the_other(ToolKind::CodexCli), ToolKind::ClaudeCli);
        assert_eq!(the_other(ToolKind::ClaudeCli), ToolKind::CodexCli);
    }

    #[test]
    fn ai_kind_string_round_trips() {
        assert_eq!(
            ai_kind_from_str(ai_kind_to_str(ToolKind::CodexCli)),
            Some(ToolKind::CodexCli)
        );
        assert_eq!(
            ai_kind_from_str(ai_kind_to_str(ToolKind::ClaudeCli)),
            Some(ToolKind::ClaudeCli)
        );
        assert_eq!(ai_kind_from_str("not_a_real_kind"), None);
    }
}
