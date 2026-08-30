import { describe, expect, it } from 'vitest'
import { applyStreamChunk, applyStreamError } from './chatEvents'
import type { ChatMessage } from './types'

function assistantMessage(): ChatMessage {
  return {
    id: 'assistant-1',
    conversation_id: 'conversation-1',
    role: 'assistant',
    content: '',
    provider_id: 'sdkmax',
    model_id: 'gpt-test',
    status: 'streaming',
    request_id: null,
    created_at: '2026-08-30T00:00:00Z',
  }
}

describe('chat stream events', () => {
  it('keeps frontend partial content aligned with interrupted database state', () => {
    const messages = [assistantMessage()]
    applyStreamChunk(messages, { message_id: 'assistant-1', content_delta: 'partial' })
    applyStreamError(messages, { message_id: 'assistant-1' })

    expect(messages).toHaveLength(1)
    expect(messages[0].content).toBe('partial')
    expect(messages[0].status).toBe('interrupted')
  })
})
