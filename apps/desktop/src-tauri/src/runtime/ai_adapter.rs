//! Phase 3 (Open Local Project Dual-AI Core Loop): the pluggable interface
//! `core_loop` uses to invoke a developer AI and an independent reviewer
//! AI, both through Phase 1's safe process-execution boundary
//! (`runtime::process`) — argument-array execution, no shell string,
//! timeout, cancellation, and redaction all reused unchanged.
//!
//! Phase 3 remediation (Blocker 2): the original single `RealCliAdapter`
//! (`<program> exec <package-path>`, the same invocation shape for both
//! Codex and Claude) is replaced by two genuinely separate adapters —
//! [`CodexCliAdapter`] and [`ClaudeCliAdapter`] — each with its own argv
//! shape, and each fronted by a real, executed (never guessed-from-memory)
//! readiness probe (see [`probe_readiness`]). [`DualCliAdapter`] composes
//! the two behind the single [`DevReviewAdapter`] trait `core_loop` drives,
//! dispatching on `request.ai_kind` so a role swap (Codex-dev/Claude-review
//! or the reverse) always uses the *correct* adapter's own argv/flags —
//! never one CLI's parameters reused for the other.
//!
//! Honesty note (see the Phase 3 remediation report's environment section):
//! neither `codex` nor `claude` is installed in every development/CI
//! environment this code ships from. The argv shapes below are built from
//! each CLI's public documentation and are marked "best effort, run-time
//! self-verifying" rather than "confirmed against a live binary" — which is
//! exactly why [`probe_readiness`] *itself executes* `--version`/`--help`
//! and scans the real output for capability keywords before ever trusting a
//! flag, rather than asserting the flag exists from memory. If a real
//! binary's actual flags differ from the best-effort defaults here, the
//! probe fails closed (`review_ready() == false` blocks starting a review;
//! `dev_ready() == false` blocks starting development) instead of silently
//! sending a wrong flag to an unattended process.
//!
//! [`MockAdapter`] is the test-only implementation used by the rest of this
//! codebase's automated tests, so the test suite never depends on an
//! actually-installed, logged-in CLI.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::detect::infra_status;
use super::process;
use super::types::{
    ExecutionErrorKind, ExecutionOutcome, ExecutionRequest, ExecutionResult, ToolKind, ToolStatus,
};

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
    /// `core_loop::write_task_package`. The adapter reads this file's
    /// *content* and sends it to the CLI as stdin, never as a raw
    /// argv/path value the CLI might otherwise treat as a literal prompt
    /// string ("C:\...\REQUIREMENT.md" is not a prompt).
    pub package_path: PathBuf,
    pub timeout: Duration,
}

/// Phase 3 remediation (Blocker 1): deliberately has **no** field carrying
/// the real project directory. `isolated_workspace_dir` is a disposable copy
/// (`core_loop::review_isolation::prepare`) — the reviewer's `cwd` is always
/// this copy, never the user's real project, so a compromised or malicious
/// reviewer process is physically unable to reach the original regardless
/// of what write access it has inside its own `cwd`.
pub struct ReviewRequest {
    pub ai_kind: ToolKind,
    pub isolated_workspace_dir: PathBuf,
    pub package_path: PathBuf,
    pub timeout: Duration,
    /// Not read by the real adapters below (the review package file itself,
    /// written by `core_loop::write_review_package`, already embeds the
    /// exact `task_id`/`result_generation`/`round` a real reviewer must
    /// echo back). Carried on the request purely so the `#[cfg(test)]`
    /// mock adapter can synthesize a matching structured-JSON verdict
    /// (`runtime::verdict`) without needing to parse the package file
    /// itself — test scripts get the exact context for free.
    #[allow(dead_code)]
    pub task_id: String,
    #[allow(dead_code)]
    pub result_generation: i64,
    #[allow(dead_code)]
    pub round: i64,
}

/// Structured review outcome — the only three verdicts the product spec
/// allows a real review round to produce. `core_loop` layers a fourth,
/// Nova-internal state (`integrity_violation`) on top of this when its own
/// technical read-only check fails; that is deliberately not a variant
/// here, since an adapter must never claim to know that on its own (only
/// the orchestrator, which controls the isolation/git snapshot comparison,
/// can determine it).
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

/// Developer/reviewer AI invocation, abstracted so `core_loop` can be driven
/// by either a real CLI pair or a deterministic test double. Methods are
/// synchronous (not `async fn`) so `dyn DevReviewAdapter` stays object safe
/// with no extra dependency (no `async-trait`) — callers invoke these from
/// inside `tokio::task::spawn_blocking`, the same pattern
/// `runtime::process::execute` itself uses internally.
pub trait DevReviewAdapter: Send + Sync {
    fn run_development(
        &self,
        request: &DevelopmentRequest,
        cancel: CancellationToken,
    ) -> ExecutionResult;
    fn run_review(&self, request: &ReviewRequest, cancel: CancellationToken) -> ExecutionResult;
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Configuration for one real CLI binary — deliberately not PATH-only:
/// `binary_path` may be a bare name (resolved via PATH, same as every other
/// Phase 1 process invocation) or a full path a user has configured, so a
/// non-PATH install location is supported without any code change.
#[derive(Debug, Clone)]
pub struct CliConfig {
    pub binary_path: String,
}

impl CliConfig {
    pub fn from_binary_name(name: &str) -> Self {
        Self {
            binary_path: name.to_string(),
        }
    }
}

/// Result of executing a real CLI's `--version`/`--help`(/subcommand
/// `--help`) probes and classifying the output. Every field is derived from
/// an actually-executed process — nothing here is asserted from memory.
#[derive(Debug, Clone)]
pub struct AdapterReadiness {
    pub tool_status: ToolStatus,
    /// Confirmed (from real `--help` output) that the CLI has some
    /// non-interactive/scriptable invocation mode Nova can drive
    /// unattended.
    pub dev_capability_confirmed: bool,
    /// Confirmed (from real `--help` output) that the CLI has some
    /// verifiable read-only/sandboxed mode. Distinct from
    /// `dev_capability_confirmed`: Blocker 1's isolated workspace is the
    /// *primary* safety boundary regardless of this flag, but per the
    /// remediation spec, if this cannot be confirmed, Nova must still
    /// refuse to start a review with this CLI rather than rely on the CLI
    /// prompt text alone ("you are READ-ONLY" is not enforcement).
    pub review_capability_confirmed: bool,
}

impl AdapterReadiness {
    /// True when `tool_status` itself is a hard blocker (not installed,
    /// not logged in, expired session, no quota, no permission, no
    /// network, failed to start, timed out, cancelled, or a bad exit code)
    /// — orthogonal to capability confirmation. A CLI can legitimately be
    /// `Unknown` (installed and ran, but its `--help` text said nothing
    /// that confirms an authenticated/login state — `runtime::detect`'s own
    /// deliberately conservative classification) while *still* having
    /// clearly documented its `exec`/`sandbox` capability in that exact
    /// same `--help` text; `Unknown` must not block capability-based
    /// readiness the way a confirmed login failure must.
    fn environment_blocks(&self) -> bool {
        matches!(
            self.tool_status,
            ToolStatus::NotFound
                | ToolStatus::NotLoggedIn
                | ToolStatus::SessionExpired
                | ToolStatus::QuotaExceeded
                | ToolStatus::PermissionDenied
                | ToolStatus::NetworkInterrupted
                | ToolStatus::StartFailed { .. }
                | ToolStatus::TimedOut
                | ToolStatus::Cancelled
                | ToolStatus::NonZeroExit { .. }
        )
    }

    pub fn dev_ready(&self) -> bool {
        !self.environment_blocks() && self.dev_capability_confirmed
    }

    pub fn review_ready(&self) -> bool {
        !self.environment_blocks() && self.review_capability_confirmed
    }

    /// A clear, actionable Chinese message naming exactly what is missing —
    /// never a generic "unavailable". `display_name` is e.g. `"Codex CLI"`.
    pub fn user_message(&self, display_name: &str) -> String {
        if self.environment_blocks() {
            return match &self.tool_status {
                ToolStatus::NotFound => format!(
                    "未检测到 {display_name} 命令行工具，请先安装并将其加入 PATH，或在设置中指定可执行文件路径。"
                ),
                ToolStatus::NotLoggedIn => {
                    format!("{display_name} 已安装但尚未登录，请先完成登录后再开始任务。")
                }
                ToolStatus::SessionExpired => {
                    format!("{display_name} 登录会话已过期，请重新登录后再开始任务。")
                }
                ToolStatus::QuotaExceeded => {
                    format!("{display_name} 当前额度不足，请检查账户额度后再重试。")
                }
                ToolStatus::PermissionDenied => {
                    format!("{display_name} 权限不足，请检查账户或系统权限后重试。")
                }
                ToolStatus::NetworkInterrupted => {
                    format!("{display_name} 网络连接失败，请检查网络后重试。")
                }
                ToolStatus::StartFailed { detail } => format!("{display_name} 启动失败：{detail}"),
                ToolStatus::TimedOut => format!("{display_name} 响应超时，请稍后重试。"),
                ToolStatus::Cancelled => format!("{display_name} 检测已取消。"),
                ToolStatus::NonZeroExit { detail, .. } => {
                    format!("{display_name} 探测命令返回非零退出码：{detail}")
                }
                ToolStatus::Ready { .. } | ToolStatus::Unknown { .. } => {
                    unreachable!("excluded by environment_blocks()")
                }
            };
        }
        if !self.dev_capability_confirmed {
            format!(
                "{display_name} 已安装，但未能确认其非交互式开发模式，暂不允许作为开发 AI 使用。"
            )
        } else if !self.review_capability_confirmed {
            format!(
                "{display_name} 已安装，但未能确认其可验证的只读审核模式，暂不允许作为审核 AI 使用（Nova 的隔离工作区仍会保护原项目，但要求 CLI 自身也具备可验证的只读能力才允许启动审核）。"
            )
        } else {
            format!("{display_name} 已就绪。")
        }
    }
}

fn run_probe(
    config: &CliConfig,
    args: &[&str],
    app_data_dir: &Path,
    cancel: &CancellationToken,
) -> ExecutionResult {
    let request = ExecutionRequest::new(
        config.binary_path.clone(),
        args.iter().map(|s| s.to_string()).collect(),
        app_data_dir.to_path_buf(),
        Some(app_data_dir.to_path_buf()),
    )
    .with_timeout(PROBE_TIMEOUT);
    process::execute_blocking(request, cancel.clone())
}

/// Reduces one probe's outcome to `(exit_code, stdout, stderr)` on success,
/// or short-circuits with the infra-level `ToolStatus` (not found, timed
/// out, ...) on a failure that never even produced classifiable output.
fn probe_ran_or_infra(
    result: &ExecutionResult,
) -> Result<(Option<i32>, String, String), ToolStatus> {
    match &result.outcome {
        ExecutionOutcome::Success => Ok((
            result.exit_code,
            result.stdout.clone(),
            result.stderr.clone(),
        )),
        ExecutionOutcome::Failed {
            kind: ExecutionErrorKind::NonZeroExit,
            ..
        } => Ok((
            result.exit_code,
            result.stdout.clone(),
            result.stderr.clone(),
        )),
        ExecutionOutcome::Failed { kind, detail } => Err(infra_status(*kind, detail)),
    }
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Real, executed readiness probe shared by both adapters — only the probe
/// *arguments* and the capability keywords differ per CLI (passed in by the
/// caller), never the control flow.
///
/// Stage 1: `<binary> --version` — proves the binary exists and starts;
/// classified via `runtime::detect`'s already-accepted `ToolStatus`
/// infra-failure mapping.
/// Stage 2: `<binary> --help` — real output run through
/// `runtime::detect::classify_tool_readiness` for login/session/quota
/// state (reusing the exact same accepted classifier Phase 1's environment
/// detection uses, not a second copy of that logic), *and* scanned for
/// `dev_keywords`/`review_keywords` substrings to confirm capability.
/// Stage 3 (optional): a further subcommand `--help` (e.g. `exec --help`
/// for Codex) whose output is also scanned for `review_keywords`, since
/// some CLIs document their sandbox/permission flags only under a
/// subcommand's own help rather than the top-level one.
fn probe_readiness(
    config: &CliConfig,
    kind: ToolKind,
    app_data_dir: &Path,
    cancel: &CancellationToken,
    dev_keywords: &[&str],
    review_keywords: &[&str],
    extra_help_args: Option<&[&str]>,
) -> AdapterReadiness {
    let version_result = run_probe(config, &["--version"], app_data_dir, cancel);
    let (_version_exit, version_stdout, _version_stderr) = match probe_ran_or_infra(&version_result)
    {
        Ok(triple) => triple,
        Err(status) => {
            return AdapterReadiness {
                tool_status: status,
                dev_capability_confirmed: false,
                review_capability_confirmed: false,
            }
        }
    };

    let help_result = run_probe(config, &["--help"], app_data_dir, cancel);
    let (help_exit, help_stdout, help_stderr) = match probe_ran_or_infra(&help_result) {
        Ok(triple) => triple,
        Err(status) => {
            return AdapterReadiness {
                tool_status: status,
                dev_capability_confirmed: false,
                review_capability_confirmed: false,
            }
        }
    };

    let tool_status =
        match super::detect::classify_tool_readiness(kind, help_exit, &help_stdout, &help_stderr) {
            ToolStatus::Ready { version: None } => ToolStatus::Ready {
                version: version_stdout.lines().next().map(str::to_string),
            },
            other => other,
        };
    let dev_capability_confirmed = dev_keywords
        .iter()
        .any(|keyword| contains_ci(&help_stdout, keyword));
    let mut review_capability_confirmed = review_keywords
        .iter()
        .any(|keyword| contains_ci(&help_stdout, keyword));

    if !review_capability_confirmed {
        if let Some(extra_args) = extra_help_args {
            let extra_result = run_probe(config, extra_args, app_data_dir, cancel);
            if let Ok((_, extra_stdout, _)) = probe_ran_or_infra(&extra_result) {
                review_capability_confirmed = review_keywords
                    .iter()
                    .any(|keyword| contains_ci(&extra_stdout, keyword));
            }
        }
    }

    AdapterReadiness {
        tool_status,
        dev_capability_confirmed,
        review_capability_confirmed,
    }
}

/// Reads `path`'s content for use as stdin — the task package's real text,
/// never the path string itself, reaches the CLI.
fn read_package_content(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

/// Production adapter for the real `codex` binary.
///
/// Best-effort argv (see this module's top-level doc comment for the
/// honesty caveat): `codex exec [--sandbox <mode>] [--cd <dir>]`, prompt
/// content via stdin. `--cd`/`-C` is included only defensively — Phase 1's
/// `ExecutionRequest.cwd` already sets the OS-level working directory
/// correctly regardless of whether this flag is right for the installed
/// version, so an unverified/wrong flag here degrades to "redundant", never
/// to "wrong directory".
pub struct CodexCliAdapter {
    pub config: CliConfig,
}

impl CodexCliAdapter {
    pub fn new(config: CliConfig) -> Self {
        Self { config }
    }

    pub fn probe(&self, app_data_dir: &Path, cancel: &CancellationToken) -> AdapterReadiness {
        probe_readiness(
            &self.config,
            ToolKind::CodexCli,
            app_data_dir,
            cancel,
            &["exec"],
            &["sandbox", "read-only", "readonly"],
            Some(&["exec", "--help"]),
        )
    }

    fn invoke(
        &self,
        cwd: &Path,
        package_path: &Path,
        sandbox_mode: &str,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        let content = read_package_content(package_path);
        let request = ExecutionRequest::new(
            self.config.binary_path.clone(),
            vec![
                "exec".to_string(),
                "--sandbox".to_string(),
                sandbox_mode.to_string(),
            ],
            cwd.to_path_buf(),
            Some(cwd.to_path_buf()),
        )
        .with_timeout(timeout)
        .with_stdin(content);
        process::execute_blocking(request, cancel)
    }
}

impl DevReviewAdapter for CodexCliAdapter {
    fn run_development(
        &self,
        request: &DevelopmentRequest,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        self.invoke(
            &request.project_dir,
            &request.package_path,
            "workspace-write",
            request.timeout,
            cancel,
        )
    }

    fn run_review(&self, request: &ReviewRequest, cancel: CancellationToken) -> ExecutionResult {
        self.invoke(
            &request.isolated_workspace_dir,
            &request.package_path,
            "read-only",
            request.timeout,
            cancel,
        )
    }
}

/// Production adapter for the real `claude` (Claude Code CLI) binary.
///
/// Best-effort argv (see this module's top-level doc comment): `claude -p
/// --permission-mode <mode>`, prompt content via stdin. Same `--cd`-style
/// caveat as [`CodexCliAdapter`] applies to any directory flag — Phase 1's
/// OS-level `cwd` is authoritative regardless.
pub struct ClaudeCliAdapter {
    pub config: CliConfig,
}

impl ClaudeCliAdapter {
    pub fn new(config: CliConfig) -> Self {
        Self { config }
    }

    pub fn probe(&self, app_data_dir: &Path, cancel: &CancellationToken) -> AdapterReadiness {
        probe_readiness(
            &self.config,
            ToolKind::ClaudeCli,
            app_data_dir,
            cancel,
            &["-p", "--print", "print"],
            &["permission-mode", "permission mode"],
            None,
        )
    }

    fn invoke(
        &self,
        cwd: &Path,
        package_path: &Path,
        permission_mode: &str,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        let content = read_package_content(package_path);
        let request = ExecutionRequest::new(
            self.config.binary_path.clone(),
            vec![
                "-p".to_string(),
                "--permission-mode".to_string(),
                permission_mode.to_string(),
            ],
            cwd.to_path_buf(),
            Some(cwd.to_path_buf()),
        )
        .with_timeout(timeout)
        .with_stdin(content);
        process::execute_blocking(request, cancel)
    }
}

impl DevReviewAdapter for ClaudeCliAdapter {
    fn run_development(
        &self,
        request: &DevelopmentRequest,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        self.invoke(
            &request.project_dir,
            &request.package_path,
            "acceptEdits",
            request.timeout,
            cancel,
        )
    }

    fn run_review(&self, request: &ReviewRequest, cancel: CancellationToken) -> ExecutionResult {
        self.invoke(
            &request.isolated_workspace_dir,
            &request.package_path,
            "plan",
            request.timeout,
            cancel,
        )
    }
}

/// Composes both real adapters behind the single `DevReviewAdapter`
/// `core_loop` drives, dispatching on `request.ai_kind` — a role swap
/// (Codex-dev/Claude-review, or the reverse) always uses the matching
/// adapter's own argv, never one CLI's parameters reused for the other.
pub struct DualCliAdapter {
    pub codex: CodexCliAdapter,
    pub claude: ClaudeCliAdapter,
}

impl DualCliAdapter {
    pub fn new(codex_config: CliConfig, claude_config: CliConfig) -> Self {
        Self {
            codex: CodexCliAdapter::new(codex_config),
            claude: ClaudeCliAdapter::new(claude_config),
        }
    }

    pub fn probe_dev(
        &self,
        kind: ToolKind,
        app_data_dir: &Path,
        cancel: &CancellationToken,
    ) -> AdapterReadiness {
        match kind {
            ToolKind::CodexCli => self.codex.probe(app_data_dir, cancel),
            ToolKind::ClaudeCli => self.claude.probe(app_data_dir, cancel),
            ToolKind::Git => unreachable!("Git is never a dev/review AI kind"),
        }
    }
}

impl DevReviewAdapter for DualCliAdapter {
    fn run_development(
        &self,
        request: &DevelopmentRequest,
        cancel: CancellationToken,
    ) -> ExecutionResult {
        match request.ai_kind {
            ToolKind::CodexCli => self.codex.run_development(request, cancel),
            ToolKind::ClaudeCli => self.claude.run_development(request, cancel),
            ToolKind::Git => unreachable!("Git is never a dev/review AI kind"),
        }
    }

    fn run_review(&self, request: &ReviewRequest, cancel: CancellationToken) -> ExecutionResult {
        match request.ai_kind {
            ToolKind::CodexCli => self.codex.run_review(request, cancel),
            ToolKind::ClaudeCli => self.claude.run_review(request, cancel),
            ToolKind::Git => unreachable!("Git is never a dev/review AI kind"),
        }
    }
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    type ReviewSideEffect = Box<dyn Fn(&ReviewRequest) + Send>;
    type DevSideEffect = Box<dyn Fn(&DevelopmentRequest) + Send>;

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
        /// writing to the project directory, or to capture the request for
        /// assertions).
        pub review_side_effect: Mutex<Option<ReviewSideEffect>>,
        pub development_side_effect: Mutex<Option<DevSideEffect>>,
        /// If set, used as the `summary` field of the synthesized
        /// structured-JSON verdict — lets a test control exactly what
        /// "review findings" text reaches the orchestrator, e.g. to prove
        /// it gets redacted before being persisted.
        pub review_findings_text: Mutex<Option<String>>,
        /// If set, overrides the synthesized verdict's raw JSON entirely —
        /// lets adversarial tests feed malformed/mismatched payloads
        /// through the exact same code path `core_loop` uses for a real
        /// reviewer's stdout.
        pub raw_review_stdout_override: Mutex<Option<String>>,
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
                development_side_effect: Mutex::new(None),
                review_findings_text: Mutex::new(None),
                raw_review_stdout_override: Mutex::new(None),
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

        /// Major 4 test support: a synthetic result shaped exactly like what
        /// Phase 1's real process boundary reports when a `CancellationToken`
        /// fires mid-run — used so `core_loop` tests can simulate "the user
        /// clicked cancel while the mock dev/review AI was running" without
        /// depending on real process timing.
        fn cancelled_result() -> ExecutionResult {
            use super::super::types::{ExecutionErrorKind, ExecutionOutcome};
            let now = chrono::Utc::now();
            ExecutionResult {
                program: "mock".to_string(),
                args: vec![],
                cwd: String::new(),
                started_at: now,
                ended_at: now,
                duration_ms: 0,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                outcome: ExecutionOutcome::Failed {
                    kind: ExecutionErrorKind::Cancelled,
                    detail: "已被用户取消".to_string(),
                },
            }
        }
    }

    impl DevReviewAdapter for MockAdapter {
        fn run_development(
            &self,
            request: &DevelopmentRequest,
            cancel: CancellationToken,
        ) -> ExecutionResult {
            self.development_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(effect) = self.development_side_effect.lock().unwrap().as_ref() {
                effect(request);
            }
            if cancel.is_cancelled() {
                return Self::cancelled_result();
            }
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
            cancel: CancellationToken,
        ) -> ExecutionResult {
            self.review_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(effect) = self.review_side_effect.lock().unwrap().as_ref() {
                effect(request);
            }
            if cancel.is_cancelled() {
                return Self::cancelled_result();
            }
            if let Some(raw) = self.raw_review_stdout_override.lock().unwrap().clone() {
                return Self::synthetic_result(true, &raw);
            }
            let mut script = self.review_script.lock().unwrap();
            let verdict = if script.is_empty() {
                ReviewVerdict::Approve
            } else {
                script.remove(0)
            };
            let summary = self
                .review_findings_text
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "Review complete.".to_string());
            let payload = serde_json::json!({
                "schema_version": crate::runtime::verdict::SCHEMA_VERSION,
                "task_id": request.task_id,
                "result_generation": request.result_generation,
                "round": request.round,
                "verdict": verdict.as_str(),
                "summary": summary,
                "findings": Vec::<String>::new(),
            });
            Self::synthetic_result(true, &payload.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // --- Blocker 2: fake-executable integration tests ----------------------
    //
    // Neither real `codex` nor `claude` is guaranteed installed in every
    // environment this test suite runs in, so these tests point
    // `CliConfig::binary_path` at `nova_test_helper`'s `fake-cli` mode (see
    // `tests/fixtures/nova_test_helper.rs`) — a real, separately-compiled
    // `.exe`, launched through the exact same Phase 1 process boundary a
    // genuine CLI would be. This verifies the *adapter's own* argv/stdin/
    // cwd/readiness-gating logic end to end, independent of whether a real
    // Codex/Claude binary happens to be present.

    use std::fs;
    use std::sync::Mutex as StdMutex;

    static FAKE_CLI_ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-ai-adapter-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn helper_exe_path() -> PathBuf {
        let mut path = std::env::current_exe().expect("current_exe for helper lookup");
        path.pop();
        if path.file_name().is_some_and(|name| name == "deps") {
            path.pop();
        }
        let exe_name = if cfg!(windows) {
            "nova_test_helper.exe"
        } else {
            "nova_test_helper"
        };
        path.push(exe_name);
        path
    }

    static HELPER_BUILD: std::sync::Once = std::sync::Once::new();

    fn helper_exe() -> String {
        let path = helper_exe_path();
        HELPER_BUILD.call_once(|| {
            if !path.is_file() {
                let status = std::process::Command::new(env!("CARGO"))
                    .args([
                        "build",
                        "--bin",
                        "nova_test_helper",
                        "--features",
                        "nova-test-helper",
                    ])
                    .current_dir(env!("CARGO_MANIFEST_DIR"))
                    .status()
                    .expect("build nova_test_helper");
                assert!(status.success());
            }
        });
        assert!(path.is_file(), "nova_test_helper missing at {path:?}");
        path.to_string_lossy().into_owned()
    }

    /// RAII guard clearing every `NOVA_FAKE_CLI_*` env var on drop, and
    /// serializing access to them — process env vars are global state, so
    /// tests that set them must not run concurrently with each other.
    struct FakeCliEnv<'a> {
        _lock: std::sync::MutexGuard<'a, ()>,
    }

    impl<'a> FakeCliEnv<'a> {
        fn set(lock: std::sync::MutexGuard<'a, ()>) -> Self {
            for key in [
                "NOVA_FAKE_CLI_VERSION_TEXT",
                "NOVA_FAKE_CLI_HELP_TEXT",
                "NOVA_FAKE_CLI_SUBHELP_TEXT",
                "NOVA_FAKE_CLI_LOG",
                "NOVA_FAKE_CLI_RESPONSE_TEXT",
                "NOVA_FAKE_CLI_RESPONSE_FILE",
                "NOVA_FAKE_CLI_STDERR_TEXT",
                "NOVA_FAKE_CLI_EXIT_CODE",
            ] {
                std::env::remove_var(key);
            }
            Self { _lock: lock }
        }
    }

    impl Drop for FakeCliEnv<'_> {
        fn drop(&mut self) {
            for key in [
                "NOVA_FAKE_CLI_VERSION_TEXT",
                "NOVA_FAKE_CLI_HELP_TEXT",
                "NOVA_FAKE_CLI_SUBHELP_TEXT",
                "NOVA_FAKE_CLI_LOG",
                "NOVA_FAKE_CLI_RESPONSE_TEXT",
                "NOVA_FAKE_CLI_RESPONSE_FILE",
                "NOVA_FAKE_CLI_STDERR_TEXT",
                "NOVA_FAKE_CLI_EXIT_CODE",
            ] {
                std::env::remove_var(key);
            }
        }
    }

    /// `nova_test_helper`'s `fake-cli` mode is normally selected by argv[0]
    /// (`"fake-cli"`), but `CodexCliAdapter`/`ClaudeCliAdapter` always send
    /// their own fixed first argv element (`"exec"`, `"-p"`). Rather than an
    /// env var (process-global mutable state — a real race against every
    /// *other* concurrently-running test in this binary that also spawns
    /// `nova_test_helper` in a non-fake-cli mode), each call here hard-links
    /// (falling back to a copy) a **fresh, uniquely-named** `...-fake-cli...`
    /// copy of the helper exe: `nova_test_helper`'s own `main` checks its
    /// *launched file's* stem for `fake-cli`, a property of which exact file
    /// was executed — immune to any other thread's environment mutations,
    /// and unique-per-call so concurrent `ai_adapter` tests never share a
    /// single renamed file either.
    fn fake_cli_config() -> CliConfig {
        let dir = temp_dir("fake-cli-copy");
        let target = dir.join(format!("nova-fake-cli-{}.exe", uuid::Uuid::new_v4()));
        let source = helper_exe();
        if fs::hard_link(&source, &target).is_err() {
            fs::copy(&source, &target).expect("copy helper exe to fake-cli-named path");
        }
        CliConfig::from_binary_name(&target.to_string_lossy())
    }

    #[test]
    fn readiness_probe_reports_not_found_for_a_genuinely_missing_binary() {
        let app_data_dir = temp_dir("readiness-missing");
        let config = CliConfig::from_binary_name(&format!(
            "nova-definitely-nonexistent-{}",
            uuid::Uuid::new_v4()
        ));
        let adapter = CodexCliAdapter::new(config);
        let readiness = adapter.probe(&app_data_dir, &CancellationToken::new());
        assert_eq!(readiness.tool_status, ToolStatus::NotFound);
        assert!(!readiness.dev_ready());
        assert!(!readiness.review_ready());
        assert!(readiness.user_message("Codex CLI").contains("未检测到"));
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn readiness_probe_confirms_dev_and_review_capability_from_real_help_output() {
        let _guard = FakeCliEnv::set(FAKE_CLI_ENV_LOCK.lock().unwrap());
        std::env::set_var("NOVA_FAKE_CLI_VERSION_TEXT", "codex-cli 9.9.9-fake");
        std::env::set_var(
            "NOVA_FAKE_CLI_HELP_TEXT",
            "Usage: codex [OPTIONS] <COMMAND>\nCommands:\n  exec  Run non-interactively\n",
        );
        std::env::set_var(
            "NOVA_FAKE_CLI_SUBHELP_TEXT",
            "Usage: codex exec [OPTIONS]\n--sandbox <MODE>  one of: read-only, workspace-write\n",
        );
        let app_data_dir = temp_dir("readiness-confirmed");
        let adapter = CodexCliAdapter::new(fake_cli_config());
        let readiness = adapter.probe(&app_data_dir, &CancellationToken::new());
        assert!(
            !matches!(readiness.tool_status, ToolStatus::NotFound),
            "{:?}",
            readiness.tool_status
        );
        assert!(readiness.dev_capability_confirmed);
        assert!(readiness.review_capability_confirmed);
        assert!(readiness.dev_ready());
        assert!(readiness.review_ready());
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn readiness_probe_refuses_review_capability_when_help_never_mentions_a_read_only_mode() {
        let _guard = FakeCliEnv::set(FAKE_CLI_ENV_LOCK.lock().unwrap());
        std::env::set_var("NOVA_FAKE_CLI_VERSION_TEXT", "claude-cli 1.0.0-fake");
        std::env::set_var(
            "NOVA_FAKE_CLI_HELP_TEXT",
            "Usage: claude [OPTIONS]\n-p, --print   run non-interactively\n",
        );
        let app_data_dir = temp_dir("readiness-no-readonly");
        let adapter = ClaudeCliAdapter::new(fake_cli_config());
        let readiness = adapter.probe(&app_data_dir, &CancellationToken::new());
        assert!(readiness.dev_capability_confirmed);
        assert!(
            !readiness.review_capability_confirmed,
            "must not confirm review-readiness without real evidence of a read-only/permission mode"
        );
        assert!(!readiness.review_ready());
        assert!(readiness
            .user_message("Claude CLI")
            .contains("只读审核模式"));
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn codex_development_invocation_sends_content_via_stdin_never_as_a_bare_path_argument() {
        let _guard = FakeCliEnv::set(FAKE_CLI_ENV_LOCK.lock().unwrap());
        let log_path = temp_dir("codex-dev-log").join("log.jsonl");
        std::env::set_var("NOVA_FAKE_CLI_LOG", log_path.to_string_lossy().into_owned());
        std::env::set_var("NOVA_FAKE_CLI_RESPONSE_TEXT", "ok");
        std::env::set_var("NOVA_FAKE_CLI_EXIT_CODE", "0");

        let project_dir = temp_dir("codex-dev-project");
        let package_path = project_dir.join("REQUIREMENT.md");
        fs::write(&package_path, "the real task package content").unwrap();

        let adapter = CodexCliAdapter::new(fake_cli_config());
        let request = DevelopmentRequest {
            ai_kind: ToolKind::CodexCli,
            project_dir: project_dir.clone(),
            package_path: package_path.clone(),
            timeout: Duration::from_secs(10),
        };
        let result = adapter.run_development(&request, CancellationToken::new());
        assert!(matches!(result.outcome, ExecutionOutcome::Success));

        let log_line = fs::read_to_string(&log_path).unwrap();
        assert!(
            log_line.contains("the real task package content"),
            "the package's actual content must reach the CLI as stdin: {log_line}"
        );
        assert!(
            !log_line.contains("REQUIREMENT.md"),
            "the file *path* must never appear in the argv the CLI receives: {log_line}"
        );
        assert!(log_line.contains("\"args\":[\"exec\""));
        assert!(log_line.contains("workspace-write"));
        let _ = fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn codex_review_invocation_uses_the_isolated_workspace_as_cwd_and_read_only_sandbox() {
        let _guard = FakeCliEnv::set(FAKE_CLI_ENV_LOCK.lock().unwrap());
        let log_path = temp_dir("codex-review-log").join("log.jsonl");
        std::env::set_var("NOVA_FAKE_CLI_LOG", log_path.to_string_lossy().into_owned());
        std::env::set_var("NOVA_FAKE_CLI_RESPONSE_TEXT", "ok");
        std::env::set_var("NOVA_FAKE_CLI_EXIT_CODE", "0");

        let workspace_dir = temp_dir("codex-review-workspace");
        let package_path = workspace_dir.join("REVIEW_REQUEST_1.md");
        fs::write(&package_path, "review package content").unwrap();

        let adapter = CodexCliAdapter::new(fake_cli_config());
        let request = ReviewRequest {
            ai_kind: ToolKind::CodexCli,
            isolated_workspace_dir: workspace_dir.clone(),
            package_path: package_path.clone(),
            timeout: Duration::from_secs(10),
            task_id: "task-1".to_string(),
            result_generation: 1,
            round: 1,
        };
        let result = adapter.run_review(&request, CancellationToken::new());
        assert!(matches!(result.outcome, ExecutionOutcome::Success));

        let log_line = fs::read_to_string(&log_path).unwrap();
        assert!(log_line.contains("read-only"));
        let canonical_workspace = fs::canonicalize(&workspace_dir).unwrap();
        assert!(
            log_line.contains(&canonical_workspace.to_string_lossy().replace('\\', "\\\\")),
            "cwd recorded by the fake CLI must be the isolated workspace: {log_line}"
        );
        let _ = fs::remove_dir_all(&workspace_dir);
    }

    #[test]
    fn claude_role_swap_uses_claudes_own_flags_never_codexs() {
        let _guard = FakeCliEnv::set(FAKE_CLI_ENV_LOCK.lock().unwrap());
        let log_path = temp_dir("claude-swap-log").join("log.jsonl");
        std::env::set_var("NOVA_FAKE_CLI_LOG", log_path.to_string_lossy().into_owned());
        std::env::set_var("NOVA_FAKE_CLI_RESPONSE_TEXT", "ok");
        std::env::set_var("NOVA_FAKE_CLI_EXIT_CODE", "0");

        let project_dir = temp_dir("claude-swap-project");
        let package_path = project_dir.join("REQUIREMENT.md");
        fs::write(&package_path, "content").unwrap();

        let dual = DualCliAdapter::new(
            CliConfig::from_binary_name("codex-should-not-be-used"),
            fake_cli_config(),
        );
        let request = DevelopmentRequest {
            ai_kind: ToolKind::ClaudeCli,
            project_dir: project_dir.clone(),
            package_path,
            timeout: Duration::from_secs(10),
        };
        let result = dual.run_development(&request, CancellationToken::new());
        assert!(matches!(result.outcome, ExecutionOutcome::Success));

        let log_line = fs::read_to_string(&log_path).unwrap();
        assert!(log_line.contains("\"args\":[\"-p\""));
        assert!(log_line.contains("acceptEdits"));
        assert!(!log_line.contains("sandbox"));
        let _ = fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn non_zero_exit_with_approve_text_embedded_is_still_reported_as_failed() {
        let _guard = FakeCliEnv::set(FAKE_CLI_ENV_LOCK.lock().unwrap());
        std::env::set_var(
            "NOVA_FAKE_CLI_RESPONSE_TEXT",
            r#"{"schema_version":1,"task_id":"task-1","result_generation":1,"round":1,"verdict":"approve","summary":"looks good"}"#,
        );
        std::env::set_var("NOVA_FAKE_CLI_EXIT_CODE", "1");

        let workspace_dir = temp_dir("nonzero-approve-workspace");
        let package_path = workspace_dir.join("REVIEW_REQUEST_1.md");
        fs::write(&package_path, "content").unwrap();
        let adapter = CodexCliAdapter::new(fake_cli_config());
        let request = ReviewRequest {
            ai_kind: ToolKind::CodexCli,
            isolated_workspace_dir: workspace_dir.clone(),
            package_path,
            timeout: Duration::from_secs(10),
            task_id: "task-1".to_string(),
            result_generation: 1,
            round: 1,
        };
        let result = adapter.run_review(&request, CancellationToken::new());
        assert!(
            !matches!(result.outcome, ExecutionOutcome::Success),
            "a non-zero exit code must never be treated as success merely because \
             APPROVE-shaped text appears in stdout"
        );
        let _ = fs::remove_dir_all(&workspace_dir);
    }

    #[test]
    fn windows_unicode_and_space_containing_project_paths_work_through_the_real_process_boundary() {
        let _guard = FakeCliEnv::set(FAKE_CLI_ENV_LOCK.lock().unwrap());
        let log_path = temp_dir("unicode-path-log").join("log.jsonl");
        std::env::set_var("NOVA_FAKE_CLI_LOG", log_path.to_string_lossy().into_owned());
        std::env::set_var("NOVA_FAKE_CLI_RESPONSE_TEXT", "ok");
        std::env::set_var("NOVA_FAKE_CLI_EXIT_CODE", "0");

        let base = temp_dir("unicode-path-base");
        let project_dir = base.join("中文 项目 空格 project");
        fs::create_dir_all(&project_dir).unwrap();
        let package_path = project_dir.join("REQUIREMENT.md");
        fs::write(&package_path, "内容 content").unwrap();

        let adapter = CodexCliAdapter::new(fake_cli_config());
        let request = DevelopmentRequest {
            ai_kind: ToolKind::CodexCli,
            project_dir: project_dir.clone(),
            package_path,
            timeout: Duration::from_secs(10),
        };
        let result = adapter.run_development(&request, CancellationToken::new());
        assert!(matches!(result.outcome, ExecutionOutcome::Success));
        let log_line = fs::read_to_string(&log_path).unwrap();
        assert!(log_line.contains("内容 content"));
        let _ = fs::remove_dir_all(&base);
    }
}
