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
