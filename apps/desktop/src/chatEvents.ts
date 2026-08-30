import type { ChatMessage } from './types'

export function applyStreamChunk(
  messages: ChatMessage[],
  event: { message_id: string; content_delta: string; request_id?: string | null },
) {
  const message = messages.find((item) => item.id === event.message_id)
  if (!message) return
  message.content += event.content_delta
  message.status = 'streaming'
  if (event.request_id) {
    message.request_id = event.request_id
  }
}

export function applyStreamError(
  messages: ChatMessage[],
  event: { message_id?: string | null; request_id?: string | null },
) {
  const message = messages.find((item) => item.id === event.message_id)
  if (!message) return
  message.status = 'interrupted'
  if (event.request_id) {
    message.request_id = event.request_id
  }
}
