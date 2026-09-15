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
