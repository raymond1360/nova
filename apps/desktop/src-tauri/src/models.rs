use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderSummary {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub masked_key: Option<String>,
    pub has_key: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SaveProviderRequest {
    pub provider_id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub object: Option<String>,
    pub owned_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub name: String,
    pub total_used: f64,
    pub total_available: f64,
    pub unlimited_quota: bool,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub provider_id: String,
    pub model_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub provider_id: String,
    pub model_id: String,
    pub status: String,
    pub request_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendChatRequest {
    pub conversation_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendChatResult {
    pub assistant_message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestConnectionResult {
    pub ok: bool,
    pub message: String,
    pub models_count: usize,
    pub request_id: Option<String>,
}

// --- Phase 2: project registry, project memory, task state machine,
// action ledger, recovery, and Phase 4 evidence-schema foundation. ---

/// Where a project's files originally came from. `github`/`template` are
/// recorded now so the schema does not need another migration once Phase
/// 3/5/6 implement Clone/template creation, but Phase 2 itself never
/// produces anything other than `local`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSourceType {
    Local,
    Github,
    Template,
}

impl ProjectSourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Github => "github",
            Self::Template => "template",
        }
    }
}

impl std::str::FromStr for ProjectSourceType {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "local" => Ok(Self::Local),
            "github" => Ok(Self::Github),
            "template" => Ok(Self::Template),
            other => Err(format!("未知项目来源类型：{other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub local_path: String,
    pub normalized_path: String,
    pub source_type: String,
    pub source_url: Option<String>,
    pub current_branch: Option<String>,
    pub last_head_summary: Option<String>,
    pub purpose: Option<String>,
    pub long_term_goal: Option<String>,
    pub user_preferences: Option<String>,
    pub prohibitions: Option<String>,
    pub last_activity_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_opened_at: Option<String>,
    /// Derived from `app_settings.last_active_project_id` at read time —
    /// deliberately not a stored column, so there is only ever one source
    /// of truth for "which project is last-active" and no risk of it
    /// drifting out of sync across rows.
    pub is_last_active: bool,
    pub data_format_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateProjectRequest {
    pub name: String,
    pub local_path: String,
    pub source_type: String,
    pub source_url: Option<String>,
    pub purpose: Option<String>,
    pub long_term_goal: Option<String>,
    pub user_preferences: Option<String>,
    pub prohibitions: Option<String>,
}

/// Result of checking whether a project's registered directory is still
/// present, unchanged, or has moved/vanished since it was last opened.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state")]
pub enum ProjectDirectoryState {
    Ok,
    Missing,
    NotADirectory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectMemory {
    pub project_id: String,
    pub product_goals: Option<String>,
    pub user_requirements: Option<String>,
    pub confirmed_decisions: Option<String>,
    pub open_questions: Option<String>,
    pub deferred_items: Option<String>,
    pub known_issues: Option<String>,
    pub task_history_summary: Option<String>,
    pub current_unfinished_task_id: Option<String>,
    pub resume_position: Option<String>,
    pub last_ai_used: Option<String>,
    pub project_rules_index: Option<String>,
    pub license_summary: Option<String>,
    pub install_run_summary: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemoryRevision {
    pub id: String,
    pub project_id: String,
    pub field_name: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub changed_by: String,
    pub reason: Option<String>,
    pub created_at: String,
}

/// Who initiated a state transition or memory change. `connected_ai` is the
/// single development AI Nova v0.1.0 talks to (Codex CLI or Claude Code
/// CLI) — there is deliberately no second "reviewing AI" actor, matching
/// the v0.1.0 single-AI runtime boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    User,
    Nova,
    ConnectedAi,
    System,
}

impl Actor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Nova => "nova",
            Self::ConnectedAi => "connected_ai",
            Self::System => "system",
        }
    }
}

impl std::str::FromStr for Actor {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "user" => Ok(Self::User),
            "nova" => Ok(Self::Nova),
            "connected_ai" => Ok(Self::ConnectedAi),
            "system" => Ok(Self::System),
            other => Err(format!("未知操作者类型：{other}")),
        }
    }
}

/// Task lifecycle state. See `storage::tasks` for the legal-transition
/// table and the invariants each transition enforces (e.g. `completed` is
/// reachable only from `awaiting_user_acceptance`, and only with a matching
/// `user_acceptance` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Draft,
    AwaitingConfirmation,
    Confirmed,
    Queued,
    Running,
    Verifying,
    SafeFixing,
    PreviewReady,
    AwaitingUserAcceptance,
    Completed,
    NeedsUserAction,
    Interrupted,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::AwaitingConfirmation => "awaiting_confirmation",
            Self::Confirmed => "confirmed",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Verifying => "verifying",
            Self::SafeFixing => "safe_fixing",
            Self::PreviewReady => "preview_ready",
            Self::AwaitingUserAcceptance => "awaiting_user_acceptance",
            Self::Completed => "completed",
            Self::NeedsUserAction => "needs_user_action",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// States in which a real external action (command execution, file
    /// write, AI invocation) may have been in flight — the set the
    /// recovery scan looks for on startup.
    pub fn is_potentially_in_flight(&self) -> bool {
        matches!(self, Self::Running | Self::Verifying | Self::SafeFixing)
    }
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TaskState {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "draft" => Ok(Self::Draft),
            "awaiting_confirmation" => Ok(Self::AwaitingConfirmation),
            "confirmed" => Ok(Self::Confirmed),
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "verifying" => Ok(Self::Verifying),
            "safe_fixing" => Ok(Self::SafeFixing),
            "preview_ready" => Ok(Self::PreviewReady),
            "awaiting_user_acceptance" => Ok(Self::AwaitingUserAcceptance),
            "completed" => Ok(Self::Completed),
            "needs_user_action" => Ok(Self::NeedsUserAction),
            "interrupted" => Ok(Self::Interrupted),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("未知任务状态：{other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub goal: Option<String>,
    pub in_scope: Option<String>,
    pub not_in_scope: Option<String>,
    pub done_criteria: Option<String>,
    pub ai_kind: Option<String>,
    pub state: TaskState,
    /// Monotonically incrementing counter, bumped every time the task
    /// produces a fresh result (enters `preview_ready`). `0` means the task
    /// has never produced a result yet — `completed` requires this to be
    /// `> 0` and a matching `accepted = 1` `user_acceptance` row for this
    /// exact generation. See `storage::tasks::bump_result_generation`.
    pub result_generation: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateTaskRequest {
    pub project_id: String,
    pub title: String,
    pub goal: Option<String>,
    pub in_scope: Option<String>,
    pub not_in_scope: Option<String>,
    pub done_criteria: Option<String>,
    pub ai_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskStateEvent {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    pub old_state: Option<TaskState>,
    pub new_state: TaskState,
    pub reason: Option<String>,
    pub actor: Actor,
    pub correlation_id: Option<String>,
    pub is_recovery: bool,
    pub safety_summary: Option<String>,
    pub created_at: String,
}

/// Lifecycle of one logical, potentially-destructive action. `uncertain` is
/// reached only by the crash-recovery sweep (an action that was `started`
/// but never reached a terminal status before the app stopped) and must
/// never be auto-retried — see `storage::actions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Planned,
    Started,
    Succeeded,
    Failed,
    Cancelled,
    Uncertain,
}

impl ActionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Started => "started",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Uncertain => "uncertain",
        }
    }
}

impl std::str::FromStr for ActionStatus {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "planned" => Ok(Self::Planned),
            "started" => Ok(Self::Started),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "uncertain" => Ok(Self::Uncertain),
            other => Err(format!("未知动作状态：{other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskAction {
    pub id: String,
    pub task_id: String,
    pub action_type: String,
    pub idempotency_key: String,
    pub attempt_number: i64,
    pub status: ActionStatus,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub evidence_ref: Option<String>,
    pub error_summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryCheckpoint {
    pub id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub detected_state: String,
    pub summary: String,
    pub risk_flag: bool,
    pub resolved_at: Option<String>,
    pub resolution: Option<String>,
    pub created_at: String,
}

/// Plain-language "what happened / what's next" summary shown to the user
/// after a restart, built from whatever `recovery_checkpoints` +
/// `task_state_events` + `task_actions` rows the recovery scan found — the
/// UI counterpart is the minimal "恢复上次工作" panel from the Phase 2 spec.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoverySummary {
    pub project_id: String,
    pub project_name: String,
    pub task: Option<Task>,
    pub last_stable_state: Option<String>,
    pub interrupted_step: Option<String>,
    pub has_risk: bool,
    pub message: String,
    pub technical_detail: Option<String>,
}

// --- Phase 4 evidence-structure foundation (schema + models only; not
// wired to any real AI/command execution in Phase 2). Every struct in this
// section is constructed only by `storage::evidence`'s own tests until
// Phase 3/4 adds real callers, hence `allow(dead_code)` throughout. ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct TimelineEvent {
    pub id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub phase: Option<String>,
    pub event_type: String,
    pub summary: String,
    pub detail: Option<String>,
    pub source: String,
    pub correlation_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct CommandEvidence {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    pub command_redacted: String,
    pub exit_code: Option<i64>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub stdout_ref: Option<String>,
    pub stderr_ref: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct GitEvidence {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    pub event_kind: String,
    pub commit_sha: Option<String>,
    pub branch: Option<String>,
    pub diff_stat: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct FileChangeEvidence {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    pub file_path: String,
    pub change_kind: String,
    pub summary: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct TestEvidence {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    pub test_command_redacted: String,
    pub passed_count: i64,
    pub failed_count: i64,
    pub skipped_count: i64,
    pub exit_code: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserAcceptance {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    /// Which of the task's results this acceptance/rejection applies to —
    /// see [`Task::result_generation`]. `UNIQUE(task_id, result_generation)`
    /// at the schema level: a task can have one acceptance record per
    /// generation, but never two for the same generation.
    pub result_generation: i64,
    pub accepted: bool,
    pub user_note: Option<String>,
    pub accepted_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct GeneratedDocument {
    pub id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub document_type: String,
    pub file_path: Option<String>,
    pub generated_by: Option<String>,
    pub generated_at: String,
    pub created_at: String,
}

// --- Phase 3: Open Local Project Dual-AI Core Loop ---

/// One independent review round for a task's current result. `verdict` is
/// stored as free text matching the schema `CHECK` constraint
/// (`approve`/`changes_required`/`insufficient_evidence`/
/// `integrity_violation`) rather than a Rust enum here, consistent with how
/// other free-form-but-constrained text columns in this codebase (e.g.
/// `nova_projects.source_type`) are modeled — the authoritative enum lives
/// with the code that produces it (`runtime::ai_adapter::ReviewVerdict`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskReview {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    pub result_generation: i64,
    pub round: i64,
    pub reviewer_ai_kind: String,
    pub verdict: String,
    pub findings_summary: Option<String>,
    pub created_at: String,
}
