import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import type { ChatMessage, Conversation, ModelInfo, ProviderSummary, TokenUsage } from '../types'

const SDKMAX_PROVIDER_ID = 'sdkmax'

export const useNovaStore = defineStore('nova', {
  state: () => ({
    activeTab: 'chat' as 'chat' | 'providers' | 'settings',
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
      if (!this.selectedModelId) {
        throw new Error('请先选择模型')
      }
      if (!this.activeConversationId) {
        await this.createConversation()
      }
      this.sending = true
      this.lastError = ''
      this.lastRequestId = ''
      const result = await invoke<{ assistant_message_id: string }>('send_chat_message', {
        request: {
          conversation_id: this.activeConversationId,
          provider_id: SDKMAX_PROVIDER_ID,
          model_id: this.selectedModelId,
          content,
        },
      })
      await this.loadMessages(this.activeConversationId!)
      return result.assistant_message_id
    },
    async stopGeneration() {
      await invoke('cancel_active_chat')
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
