export interface ProviderSummary {
  id: string
  name: string
  base_url: string
  masked_key: string | null
  has_key: boolean
  updated_at: string | null
}

export interface ModelInfo {
  id: string
  object?: string
  created?: number
  owned_by?: string
  supported_endpoint_types?: string[]
}

export interface TokenUsage {
  name: string
  total_granted: number
  total_used: number
  total_available: number
  unlimited_quota: boolean
  expires_at: number
}

export interface Conversation {
  id: string
  title: string
  provider_id: string
  model_id: string | null
  created_at: string
  updated_at: string
}

export type MessageRole = 'user' | 'assistant' | 'system'

export interface ChatMessage {
  id: string
  conversation_id: string
  role: MessageRole
  content: string
  provider_id: string
  model_id: string | null
  status: 'complete' | 'streaming' | 'interrupted' | 'error'
  request_id: string | null
  created_at: string
}

export interface AppError {
  code: string
  message: string
  request_id?: string | null
  diagnostic?: string | null
}

// Mirrors src-tauri/src/runtime/types.rs::ToolStatus (serde tag = "state").
export type ToolStatus =
  | { state: 'not_found' }
  | { state: 'ready'; version: string | null }
  | { state: 'not_logged_in' }
  | { state: 'session_expired' }
  | { state: 'quota_exceeded' }
  | { state: 'permission_denied' }
  | { state: 'network_interrupted' }
  | { state: 'start_failed'; detail: string }
  | { state: 'non_zero_exit'; exit_code: number | null; detail: string }
  | { state: 'timed_out' }
  | { state: 'cancelled' }
  | { state: 'unknown'; detail: string }

// Mirrors src-tauri/src/runtime/detect.rs::DirectoryStatus.
export type DirectoryStatus =
  | { state: 'ready'; path: string }
  | { state: 'not_found' }
  | { state: 'not_a_directory' }
  | { state: 'permission_denied' }
  | { state: 'cleanup_failed'; detail: string }
  | { state: 'unknown'; detail: string }

export interface PlatformStatus {
  os: string
  arch: string
  is_supported: boolean
}

export interface RuntimeEnvironmentReport {
  platform: PlatformStatus
  git: ToolStatus
  codex_cli: ToolStatus
  claude_cli: ToolStatus
  app_data_dir: DirectoryStatus
  generated_at: string
}

// --- Phase 3: Open Local Project Dual-AI Core Loop ---

export interface Project {
  id: string
  name: string
  local_path: string
  normalized_path: string
  source_type: string
  source_url: string | null
  current_branch: string | null
  last_head_summary: string | null
  purpose: string | null
  long_term_goal: string | null
  user_preferences: string | null
  prohibitions: string | null
  last_activity_at: string | null
  created_at: string
  updated_at: string
  last_opened_at: string | null
}

export type TaskState =
  | 'draft'
  | 'awaiting_confirmation'
  | 'confirmed'
  | 'queued'
  | 'running'
  | 'verifying'
  | 'safe_fixing'
  | 'preview_ready'
  | 'awaiting_user_acceptance'
  | 'completed'
  | 'needs_user_action'
  | 'interrupted'
  | 'failed'
  | 'cancelled'

export interface Task {
  id: string
  project_id: string
  title: string
  goal: string | null
  in_scope: string | null
  not_in_scope: string | null
  done_criteria: string | null
  ai_kind: string | null
  state: TaskState
  result_generation: number
  test_command_redacted: string | null
  test_command_source: 'auto' | 'user' | null
  document_sync_pending: boolean
  created_at: string
  updated_at: string
}

export interface TaskStateEvent {
  id: string
  project_id: string
  task_id: string
  old_state: TaskState | null
  new_state: TaskState
  reason: string | null
  actor: string
  created_at: string
}

export interface TaskReview {
  id: string
  project_id: string
  task_id: string
  result_generation: number
  round: number
  reviewer_ai_kind: string
  verdict: 'approve' | 'changes_required' | 'insufficient_evidence' | 'integrity_violation'
  findings_summary: string | null
  created_at: string
}

export interface CoreLoopReadiness {
  dev_kind: string
  reviewer_kind: string
  dev_ready: boolean
  review_ready: boolean
  dev_message: string
  review_message: string
}

// --- Second re-review remediation ---

export interface NoTestsConfirmation {
  id: string
  project_id: string
  task_id: string
  result_generation: number
  round: number
  confirmed_by: string
  reason: string | null
  confirmed_at: string
  created_at: string
}

export interface NoBuildConfirmation {
  id: string
  project_id: string
  task_id: string
  result_generation: number
  round: number
  confirmed_by: string
  reason: string | null
  confirmed_at: string
  created_at: string
}

export interface CliBinaryTrustStatus {
  trusted: boolean
  canonical_path: string | null
  file_size: number | null
  sha256_hex: string | null
  confirmed_at: string | null
  mismatch_reason: string | null
}

// --- Phase 4: evidence and document gate ---
// Mirrors src-tauri/src/core_loop/evidence_overview.rs's serde output.

export interface TimelineEntry {
  id: string
  event_type: string
  title: string
  status: string
  started_at: string | null
  finished_at: string | null
  result_generation: number | null
  round: number | null
  needs_user_action: boolean
  is_historical: boolean
}

export interface TestEvidence {
  id: string
  project_id: string
  task_id: string
  result_generation: number
  round: number
  test_command_redacted: string | null
  status: string
  passed_count: number
  failed_count: number
  skipped_count: number
  exit_code: number | null
  reason: string | null
  stdout_ref: string | null
  stderr_ref: string | null
  created_at: string
}

export interface TestEvidenceEntry extends TestEvidence {
  is_current_generation: boolean
  is_current_attempt: boolean
}

export interface BuildEvidence {
  id: string
  project_id: string
  task_id: string
  result_generation: number
  round: number
  build_command_redacted: string | null
  status: string
  exit_code: number | null
  reason: string | null
  stdout_ref: string | null
  stderr_ref: string | null
  created_at: string
}

export interface BuildEvidenceEntry extends BuildEvidence {
  is_current_generation: boolean
  is_current_attempt: boolean
}

export interface CommandEvidence {
  id: string
  project_id: string
  task_id: string
  command_redacted: string
  exit_code: number | null
  started_at: string | null
  finished_at: string | null
  stdout_ref: string | null
  stderr_ref: string | null
  created_at: string
}

export interface GitEvidence {
  id: string
  project_id: string
  task_id: string
  event_kind: string
  commit_sha: string | null
  branch: string | null
  diff_stat: string | null
  created_at: string
}

export interface FileChangeEvidence {
  id: string
  project_id: string
  task_id: string
  file_path: string
  change_kind: 'created' | 'modified' | 'deleted'
  summary: string | null
  created_at: string
}

export interface UserAcceptanceRecord {
  id: string
  project_id: string
  task_id: string
  result_generation: number
  accepted: boolean
  user_note: string | null
  accepted_at: string
  created_at: string
}

export interface TaskEvidenceOverview {
  project_id: string
  task_id: string
  task_state: TaskState
  result_generation: number
  pending_round: number
  needs_user_action: boolean
  acceptance_ready: boolean
  is_completed: boolean
  timeline: TimelineEntry[]
  file_changes: FileChangeEvidence[]
  git_events: GitEvidence[]
  commands: CommandEvidence[]
  tests: TestEvidenceEntry[]
  builds: BuildEvidenceEntry[]
  acceptance_history: UserAcceptanceRecord[]
}

// --- WP2A: self-check contract + completion gate ---
// Mirrors src-tauri/src/core_loop.rs::CompletionPrecheck and
// src-tauri/src/models.rs::SelfCheck's serde output.

export interface CompletionPrecheck {
  ok: boolean
  can_submit_acceptance: boolean
  reason_code: string | null
  message: string | null
}

export interface SelfCheck {
  id: string
  project_id: string
  task_id: string
  round: number
  result_attempt: number
  parse_status: 'valid' | 'invalid_format'
  self_reported_status: 'self_reported_complete' | 'self_reported_incomplete' | 'self_reported_blocked' | null
  requirement_coverage_json: string | null
  check_results_json: string | null
  unfinished_items_json: string | null
  blocking_issues_json: string | null
  raw_block_redacted: string
  created_at: string
}

export interface TaskDocumentInfo {
  document_type: string
  display_name: string
  exists: boolean
  is_optional_diagnostic: boolean
}

export interface TaskDocumentContent {
  document_type: string
  display_name: string
  exists: boolean
  content: string | null
  error: string | null
}
