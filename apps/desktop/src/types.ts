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
