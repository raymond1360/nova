use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Which external tool a detection/execution request targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Git,
    CodexCli,
    ClaudeCli,
}

/// Structured readiness state for an external CLI tool.
///
/// This must never be inferred from a bare `--version` exit code alone.
/// Every variant is reached through a real, classified process-execution
/// outcome in `runtime::detect` (spawn failure kind, exit code, timeout,
/// or cancellation). Variants that describe account/session/quota state
/// (`NotLoggedIn`, `SessionExpired`, `QuotaExceeded`, `NetworkInterrupted`)
/// are part of the state model required by the Stage 0 gap matrix, but are
/// only ever produced by a detector when it has real, verifiable evidence
/// for that specific tool's behavior; otherwise a non-zero exit or unusual
/// failure is reported as `NonZeroExit`/`Unknown` with redacted detail
/// rather than guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ToolStatus {
    /// Executable not found on PATH (or configured location).
    NotFound,
    /// Executable launched and returned a usable result (e.g. `--version` exit 0).
    Ready { version: Option<String> },
    /// Executable ran but reported the user is not logged in.
    NotLoggedIn,
    /// Executable ran but reported an expired/invalid session.
    SessionExpired,
    /// Executable ran but reported insufficient quota.
    QuotaExceeded,
    /// Executable ran but reported insufficient permission for the requested action/directory.
    PermissionDenied,
    /// Executable ran but the request could not complete because of a network interruption.
    NetworkInterrupted,
    /// The OS could not start the process at all (distinct from "not found").
    StartFailed { detail: String },
    /// Process started and exited with a non-zero, unclassified code.
    NonZeroExit {
        exit_code: Option<i32>,
        detail: String,
    },
    /// Process did not finish within the configured timeout and was terminated.
    TimedOut,
    /// Detection was cancelled by the user before completion.
    Cancelled,
    /// Process failed in a way that does not match any classified state above.
    Unknown { detail: String },
}

/// Classification of why a process execution did not run to a clean,
/// known-good completion. This is the low-level counterpart to
/// [`ToolStatus`]: detectors map an [`ExecutionOutcome`] into the richer,
/// tool-facing [`ToolStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionErrorKind {
    NotFound,
    PermissionDenied,
    StartFailed,
    TimedOut,
    Cancelled,
    NonZeroExit,
    PathRejected,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ExecutionOutcome {
    Success,
    Failed {
        kind: ExecutionErrorKind,
        detail: String,
    },
}

/// A request to run exactly one external program under Nova's safety
/// boundary: a program path plus an argument array, never a shell command
/// string. See `runtime::process::execute`.
#[derive(Debug, Clone)]
pub struct ExecutionRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Directory the resolved `cwd` must stay inside (after canonicalization).
    /// `None` means no user-selected directory is allowed for this request —
    /// only Nova's own app data directory logic applies upstream.
    pub allowed_root: Option<PathBuf>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl ExecutionRequest {
    pub fn new(
        program: impl Into<String>,
        args: Vec<String>,
        cwd: PathBuf,
        allowed_root: Option<PathBuf>,
    ) -> Self {
        Self {
            program: program.into(),
            args,
            cwd,
            allowed_root,
            timeout: Duration::from_secs(20),
            max_output_bytes: 64 * 1024,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    /// Redacted, size-capped standard output.
    pub stdout: String,
    /// Redacted, size-capped standard error.
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub outcome: ExecutionOutcome,
}
