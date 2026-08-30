<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useNovaStore } from './stores/nova'
import { applyStreamChunk, applyStreamError } from './chatEvents'
import type { ChatMessage } from './types'

const store = useNovaStore()
const providerName = ref('SDKMAX')
const baseUrl = ref('https://api.sdkmax.com')
const apiKey = ref('')
const modelSearch = ref('')
const prompt = ref('')
const statusText = ref('')
const usageLoading = ref(false)
const modelLoading = computed(() => store.loadingModels)

let unlistenChunk: UnlistenFn | null = null
let unlistenDone: UnlistenFn | null = null
let unlistenError: UnlistenFn | null = null

const filteredModels = computed(() => {
  const query = modelSearch.value.trim().toLowerCase()
  if (!query) return store.models
  return store.models.filter((model) => model.id.toLowerCase().includes(query))
})

const messages = computed<ChatMessage[]>(() => store.messages)

onMounted(async () => {
  await store.initialize()
  providerName.value = store.provider?.name ?? 'SDKMAX'
  baseUrl.value = store.provider?.base_url ?? 'https://api.sdkmax.com'
  unlistenChunk = await listen<{ message_id: string; content_delta: string; request_id?: string | null }>(
    'chat://chunk',
    (event) => {
      applyStreamChunk(store.messages, event.payload)
      if (event.payload.request_id) {
        store.lastRequestId = event.payload.request_id
      }
    },
  )
  unlistenDone = await listen<{ message_id: string; status: string; request_id?: string | null }>(
    'chat://done',
    async (event) => {
      if (event.payload.request_id) store.lastRequestId = event.payload.request_id
      await store.finishSending()
    },
  )
  unlistenError = await listen<{ message_id?: string | null; message: string; request_id?: string | null }>(
    'chat://error',
    async (event) => {
      store.lastError = event.payload.message
      applyStreamError(store.messages, event.payload)
      if (event.payload.request_id) store.lastRequestId = event.payload.request_id
      await store.finishSending()
    },
  )
})

onUnmounted(() => {
  unlistenChunk?.()
  unlistenDone?.()
  unlistenError?.()
})

function onModelChange(event: Event) {
  store.selectModel((event.target as HTMLSelectElement).value)
}

async function saveProvider() {
  statusText.value = ''
  await store.saveProvider(providerName.value, baseUrl.value, apiKey.value)
  apiKey.value = ''
  statusText.value = '供应商已保存'
}

async function testConnection() {
  statusText.value = ''
  const result = await store.testConnection()
  statusText.value = `${result.message}，模型 ${result.models_count} 个`
  await store.refreshModels()
}

async function refreshUsage() {
  usageLoading.value = true
  try {
    await store.refreshUsage()
  } finally {
    usageLoading.value = false
  }
}

async function send() {
  const text = prompt.value.trim()
  if (!text) return
  prompt.value = ''
  await store.sendMessage(text)
}
</script>

<template>
  <main class="shell">
    <aside class="sidebar">
      <div class="brand">
        <span class="brand-mark">N</span>
        <div>
          <h1>Nova</h1>
          <p>SDKMAX 技术 MVP</p>
        </div>
      </div>
      <nav class="nav">
        <button :class="{ active: store.activeTab === 'chat' }" @click="store.activeTab = 'chat'">聊天</button>
        <button :class="{ active: store.activeTab === 'providers' }" @click="store.activeTab = 'providers'">
          供应商
        </button>
        <button :class="{ active: store.activeTab === 'settings' }" @click="store.activeTab = 'settings'">设置</button>
      </nav>
      <section class="conversation-list">
        <div class="row">
          <strong>会话</strong>
          <button class="icon-button" title="新建会话" @click="store.createConversation">+</button>
        </div>
        <button
          v-for="conversation in store.conversations"
          :key="conversation.id"
          class="conversation"
          :class="{ active: conversation.id === store.activeConversationId }"
          @click="store.loadMessages(conversation.id)"
        >
          <span>{{ conversation.title }}</span>
          <small>{{ conversation.model_id || '未选模型' }}</small>
        </button>
      </section>
    </aside>

    <section v-if="store.activeTab === 'chat'" class="page chat-page">
      <header class="toolbar">
        <div class="model-picker">
          <input v-model="modelSearch" placeholder="搜索模型" />
          <select :value="store.selectedModelId" @change="onModelChange">
            <option value="">选择模型</option>
            <option v-for="model in filteredModels" :key="model.id" :value="model.id">{{ model.id }}</option>
          </select>
        </div>
        <button @click="store.refreshModels" :disabled="modelLoading || !store.provider?.has_key">
          {{ modelLoading ? '刷新中' : '刷新模型' }}
        </button>
      </header>
      <div class="messages">
        <article v-for="message in messages" :key="message.id" class="message" :class="message.role">
          <header>
            <strong>{{ message.role === 'user' ? '你' : 'Nova' }}</strong>
            <span v-if="message.status === 'interrupted'">未完成</span>
            <span v-if="message.status === 'error'">出错</span>
          </header>
          <p>{{ message.content }}</p>
          <small v-if="message.request_id">request_id: {{ message.request_id }}</small>
        </article>
      </div>
      <footer class="composer">
        <textarea v-model="prompt" placeholder="输入消息" @keydown.ctrl.enter.prevent="send" />
        <button v-if="!store.sending" @click="send">发送</button>
        <button v-else @click="store.stopGeneration">停止</button>
      </footer>
      <p v-if="store.lastError" class="error">{{ store.lastError }}</p>
      <p v-if="store.lastRequestId" class="diagnostic">request_id: {{ store.lastRequestId }}</p>
    </section>

    <section v-if="store.activeTab === 'providers'" class="page">
      <header class="page-header">
        <h2>供应商</h2>
        <p>API Key 只交给 Rust 保存，不进入前端持久状态。</p>
      </header>
      <div class="form-grid">
        <label>
          名称
          <input v-model="providerName" />
        </label>
        <label>
          Base URL
          <input v-model="baseUrl" />
        </label>
        <label>
          API Key
          <input v-model="apiKey" type="password" autocomplete="off" placeholder="输入后保存到系统凭据" />
        </label>
        <div class="provider-summary">
          <span>当前凭据</span>
          <strong>{{ store.provider?.masked_key || '未保存' }}</strong>
        </div>
      </div>
      <div class="actions">
        <button @click="testConnection" :disabled="!store.provider?.has_key">测试连接</button>
        <button @click="saveProvider">保存</button>
        <button class="danger" @click="store.deleteProviderCredential">删除凭据</button>
        <button @click="refreshUsage" :disabled="usageLoading || !store.provider?.has_key">
          {{ usageLoading ? '刷新中' : '刷新用量' }}
        </button>
      </div>
      <p v-if="statusText" class="success">{{ statusText }}</p>
      <section v-if="store.tokenUsage" class="usage">
        <div><span>Token 名称</span><strong>{{ store.tokenUsage.name }}</strong></div>
        <div><span>已用额度</span><strong>{{ store.tokenUsage.total_used }}</strong></div>
        <div><span>可用额度</span><strong>{{ store.tokenUsage.total_available }}</strong></div>
        <div><span>无限额度</span><strong>{{ store.tokenUsage.unlimited_quota ? '是' : '否' }}</strong></div>
        <div><span>过期时间</span><strong>{{ store.tokenUsage.expires_at || '永不过期' }}</strong></div>
      </section>
    </section>

    <section v-if="store.activeTab === 'settings'" class="page">
      <header class="page-header">
        <h2>设置</h2>
        <p>本地 MVP 仅开放必要设置。</p>
      </header>
      <div class="settings-list">
        <div><span>应用标识</span><strong>com.sdkmax.nova</strong></div>
        <div><span>平台</span><strong>Windows x64</strong></div>
        <div><span>最近模型</span><strong>{{ store.selectedModelId || '未选择' }}</strong></div>
        <div><span>供应商</span><strong>{{ store.provider?.base_url }}</strong></div>
      </div>
    </section>
  </main>
</template>
