import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import type {
  ChatMessage,
  Conversation,
  ModelInfo,
  ProviderSummary,
  RuntimeEnvironmentReport,
  TokenUsage,
} from '../types'

const SDKMAX_PROVIDER_ID = 'sdkmax'

export const useNovaStore = defineStore('nova', {
  state: () => ({
    activeTab: 'chat' as 'chat' | 'providers' | 'settings' | 'environment' | 'coreloop',
    provider: null as ProviderSummary | null,
    models: [] as ModelInfo[],
    tokenUsage: null as TokenUsage | null,
    conversations: [] as Conversation[],
    activeConversationId: null as string | null,
    messages: [] as ChatMessage[],
    selectedModelId: '',
    loadingModels: false,
    sending: false,
    lastError: '',
    lastRequestId: '',
    environment: null as RuntimeEnvironmentReport | null,
    environmentLoading: false,
    environmentError: '',
  }),
  getters: {
    activeConversation(state) {
      return state.conversations.find((item) => item.id === state.activeConversationId) ?? null
    },
    modelIds(state) {
      return state.models.map((model) => model.id)
    },
  },
  actions: {
    async initialize() {
      await invoke('initialize_app')
      await this.refreshProvider()
      await this.loadConversations()
      const savedModel = await invoke<string | null>('get_setting', { key: 'last_model_id' })
      this.selectedModelId = savedModel ?? ''
    },
    async refreshProvider() {
      this.provider = await invoke<ProviderSummary>('get_provider', { providerId: SDKMAX_PROVIDER_ID })
    },
    async saveProvider(name: string, baseUrl: string, apiKey: string) {
      this.provider = await invoke<ProviderSummary>('save_provider', {
        request: { provider_id: SDKMAX_PROVIDER_ID, name, base_url: baseUrl, api_key: apiKey },
      })
    },
    async deleteProviderCredential() {
      this.provider = await invoke<ProviderSummary>('delete_provider_credential', {
        providerId: SDKMAX_PROVIDER_ID,
      })
      this.models = []
      this.tokenUsage = null
    },
    async refreshModels() {
      this.loadingModels = true
      this.lastError = ''
      try {
        this.models = await invoke<ModelInfo[]>('fetch_models', { providerId: SDKMAX_PROVIDER_ID })
        if (!this.selectedModelId && this.models[0]) {
          await this.selectModel(this.models[0].id)
        }
        if (this.selectedModelId && !this.models.some((model) => model.id === this.selectedModelId)) {
          await this.selectModel(this.models[0]?.id ?? '')
        }
      } catch (error) {
        this.lastError = String(error)
        throw error
      } finally {
        this.loadingModels = false
      }
    },
    async testConnection() {
      return await invoke<{ ok: boolean; message: string; models_count: number; request_id?: string | null }>(
        'test_provider_connection',
        { providerId: SDKMAX_PROVIDER_ID },
      )
    },
    async refreshUsage() {
      this.tokenUsage = await invoke<TokenUsage>('fetch_token_usage', { providerId: SDKMAX_PROVIDER_ID })
    },
    async selectModel(modelId: string) {
      this.selectedModelId = modelId
      await invoke('set_setting', { key: 'last_model_id', value: modelId })
    },
    async loadConversations() {
      this.conversations = await invoke<Conversation[]>('list_conversations')
      if (!this.activeConversationId) {
        this.activeConversationId = this.conversations[0]?.id ?? null
      }
      if (this.activeConversationId) {
        await this.loadMessages(this.activeConversationId)
      }
    },
    async createConversation() {
      const conversation = await invoke<Conversation>('create_conversation', {
        providerId: SDKMAX_PROVIDER_ID,
        modelId: this.selectedModelId || null,
      })
      this.conversations = [conversation, ...this.conversations]
      this.activeConversationId = conversation.id
      this.messages = []
    },
    async deleteConversation(conversationId: string) {
      await invoke('delete_conversation', { conversationId })
      if (this.activeConversationId === conversationId) {
        this.activeConversationId = null
        this.messages = []
      }
      await this.loadConversations()
    },
    async loadMessages(conversationId: string) {
      this.activeConversationId = conversationId
      this.messages = await invoke<ChatMessage[]>('list_messages', { conversationId })
    },
    async sendMessage(content: string) {
      // Reentrancy guard: a stream is already in flight (either actively
      // streaming, or between invoke() and the backend emitting its first
      // chat:// event). Silently no-op instead of starting a second
      // request that would fight the first one over the single Rust-side
      // active_cancel slot. This must be the very first thing this
      // function does, before any `await`, so a second synchronous call
      // (e.g. Ctrl+Enter fired twice) cannot slip in during the
      // createConversation() gap below.
      if (this.sending) {
        return
      }
      if (!this.selectedModelId) {
        throw new Error('请先选择模型')
      }
      this.sending = true
      this.lastError = ''
      this.lastRequestId = ''
      let assistantMessageId: string
      try {
        if (!this.activeConversationId) {
          await this.createConversation()
        }
        const result = await invoke<{ assistant_message_id: string }>('send_chat_message', {
          request: {
            conversation_id: this.activeConversationId,
            provider_id: SDKMAX_PROVIDER_ID,
            model_id: this.selectedModelId,
            content,
          },
        })
        assistantMessageId = result.assistant_message_id
      } catch (error) {
        // Nothing was started on the Rust side (create_conversation or
        // send_chat_message itself rejected), so no chat://done or
        // chat://error will ever arrive for this attempt. This function
        // is the only owner of `sending` on this failure path — reset it
        // here so the user can retry immediately. The error message is
        // already redacted by the Rust command boundary before it ever
        // reaches this catch block.
        this.sending = false
        this.lastError = String(error)
        throw error
      }
      // From this point on, Rust has accepted the request and spawned the
      // background stream task. Ownership of `sending` now belongs to
      // finishSending() (invoked by the chat://done / chat://error
      // listeners in App.vue) — do not reset it here even if this
      // best-effort refresh fails, or a still-running stream would look
      // finished and let a second send race with it.
      await this.loadMessages(this.activeConversationId!)
      return assistantMessageId
    },
    async stopGeneration() {
      await invoke('cancel_active_chat')
    },
    async detectEnvironment() {
      // Reentrancy guard, same pattern as sendMessage(): the backend also
      // serializes detection runs and rejects a second concurrent call, but
      // guarding here too means a rapid double-click never even reaches the
      // backend, and never risks one call's `environmentLoading = false` in
      // `finally` clobbering the state of a different, still-running call.
      if (this.environmentLoading) {
        return
      }
      this.environmentLoading = true
      this.environmentError = ''
      try {
        this.environment = await invoke<RuntimeEnvironmentReport>('detect_runtime_environment')
      } catch (error) {
        this.environmentError = String(error)
        throw error
      } finally {
        this.environmentLoading = false
      }
    },
    async cancelEnvironmentDetection() {
      await invoke('cancel_environment_detection')
    },
    async finishSending() {
      this.sending = false
      if (this.activeConversationId) {
        await this.loadMessages(this.activeConversationId)
      }
      await this.loadConversations()
    },
  },
})
