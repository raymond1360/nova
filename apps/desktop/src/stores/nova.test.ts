import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}))

import { invoke } from '@tauri-apps/api/core'
import { useNovaStore } from './nova'

const mockedInvoke = vi.mocked(invoke)

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

/** Routes a mocked `invoke()` call to a handler keyed by Tauri command name. */
function routeInvoke(handlers: Record<string, (args: unknown) => unknown>) {
  mockedInvoke.mockImplementation(async (command: string, args?: unknown) => {
    if (!(command in handlers)) {
      throw new Error(`unexpected invoke: ${command}`)
    }
    return handlers[command](args)
  })
}

describe('nova store sendMessage reentrancy', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    mockedInvoke.mockReset()
  })

  it('only creates one conversation and calls send_chat_message once when invoked twice back-to-back with no active conversation', async () => {
    const store = useNovaStore()
    store.selectedModelId = 'gpt-test'

    const createGate = deferred<void>()
    let createConversationCalls = 0
    let sendChatCalls = 0

    routeInvoke({
      create_conversation: async () => {
        createConversationCalls += 1
        await createGate.promise
        return {
          id: 'conv-1',
          title: '新会话',
          provider_id: 'sdkmax',
          model_id: 'gpt-test',
          created_at: 'now',
          updated_at: 'now',
        }
      },
      send_chat_message: async () => {
        sendChatCalls += 1
        return { assistant_message_id: 'assistant-1' }
      },
      list_messages: async () => [],
      list_conversations: async () => [],
    })

    // Fire twice back-to-back, without awaiting the first — this is the
    // exact race window the fix must close: the first call is suspended
    // inside createConversation() (createGate not yet resolved) when the
    // second call happens.
    const first = store.sendMessage('first message')
    const second = store.sendMessage('second message')

    // The reentrancy guard must have already flipped `sending` to true
    // synchronously before the second call even ran, so the second call
    // should resolve immediately without touching invoke again.
    await second

    expect(createConversationCalls).toBe(1)
    expect(sendChatCalls).toBe(0) // first attempt hasn't reached send_chat_message yet

    createGate.resolve()
    await first

    expect(createConversationCalls).toBe(1)
    expect(sendChatCalls).toBe(1)
  })

  it('rejects a second sendMessage call while the first is still awaiting createConversation', async () => {
    const store = useNovaStore()
    store.selectedModelId = 'gpt-test'

    const createGate = deferred<void>()
    routeInvoke({
      create_conversation: async () => {
        await createGate.promise
        return {
          id: 'conv-1',
          title: '新会话',
          provider_id: 'sdkmax',
          model_id: 'gpt-test',
          created_at: 'now',
          updated_at: 'now',
        }
      },
      send_chat_message: async () => ({ assistant_message_id: 'assistant-1' }),
      list_messages: async () => [],
      list_conversations: async () => [],
    })

    const first = store.sendMessage('first')
    expect(store.sending).toBe(true)

    const second = await store.sendMessage('second')
    expect(second).toBeUndefined()
    expect(store.activeConversationId).toBeNull() // second call made no progress at all

    createGate.resolve()
    await first
  })

  it('restores sending to false and allows retry when createConversation fails', async () => {
    const store = useNovaStore()
    store.selectedModelId = 'gpt-test'

    routeInvoke({
      create_conversation: async () => {
        throw new Error('本地数据库锁定失败')
      },
    })

    await expect(store.sendMessage('hello')).rejects.toThrow('本地数据库锁定失败')
    expect(store.sending).toBe(false)
    expect(store.lastError).toContain('本地数据库锁定失败')

    // Retry must actually be allowed, not permanently blocked by a stuck flag.
    routeInvoke({
      create_conversation: async () => ({
        id: 'conv-2',
        title: '新会话',
        provider_id: 'sdkmax',
        model_id: 'gpt-test',
        created_at: 'now',
        updated_at: 'now',
      }),
      send_chat_message: async () => ({ assistant_message_id: 'assistant-2' }),
      list_messages: async () => [],
      list_conversations: async () => [],
    })
    const assistantId = await store.sendMessage('hello again')
    expect(assistantId).toBe('assistant-2')
    expect(store.sending).toBe(true) // still "sending" until finishSending() runs
  })

  it('restores sending to false with a redacted message when send_chat_message itself fails', async () => {
    const store = useNovaStore()
    store.selectedModelId = 'gpt-test'
    store.activeConversationId = 'conv-existing'

    routeInvoke({
      send_chat_message: async () => {
        // Rust command boundary already redacts errors before this
        // rejects (see error.rs::public_error) — the frontend must not
        // need to do additional redaction to pass this through.
        throw new Error('API Key 无效或已过期')
      },
    })

    await expect(store.sendMessage('hello')).rejects.toThrow('API Key 无效或已过期')
    expect(store.sending).toBe(false)
    expect(store.lastError).toContain('API Key 无效或已过期')
    expect(store.lastError).not.toMatch(/sk-[A-Za-z0-9_-]{8,}/)
  })

  it('does not disturb the normal send flow: sending stays true after invoke succeeds, until finishSending runs', async () => {
    const store = useNovaStore()
    store.selectedModelId = 'gpt-test'
    store.activeConversationId = 'conv-existing'

    routeInvoke({
      send_chat_message: async () => ({ assistant_message_id: 'assistant-3' }),
      list_messages: async () => [
        {
          id: 'assistant-3',
          conversation_id: 'conv-existing',
          role: 'assistant',
          content: '',
          provider_id: 'sdkmax',
          model_id: 'gpt-test',
          status: 'streaming',
          request_id: null,
          created_at: 'now',
        },
      ],
    })

    const assistantId = await store.sendMessage('hello')
    expect(assistantId).toBe('assistant-3')
    expect(store.sending).toBe(true)
    expect(store.messages).toHaveLength(1)

    // Simulate the chat://done event handler that App.vue wires up.
    routeInvoke({
      list_messages: async () => [],
      list_conversations: async () => [],
    })
    await store.finishSending()
    expect(store.sending).toBe(false)
  })

  it('allows a normal send again after the user cancels a previous one', async () => {
    const store = useNovaStore()
    store.selectedModelId = 'gpt-test'
    store.activeConversationId = 'conv-existing'

    routeInvoke({
      send_chat_message: async () => ({ assistant_message_id: 'assistant-4' }),
      list_messages: async () => [],
      list_conversations: async () => [],
    })

    await store.sendMessage('first')
    expect(store.sending).toBe(true)

    // Backend emits chat://error with user_canceled, App.vue's listener
    // calls finishSending() (unchanged behavior).
    await store.finishSending()
    expect(store.sending).toBe(false)

    // A brand-new send must be accepted, not silently swallowed by a
    // stuck reentrancy guard.
    const secondId = await store.sendMessage('second')
    expect(secondId).toBe('assistant-4')
  })
})
