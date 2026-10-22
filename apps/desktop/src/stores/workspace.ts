import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'

export interface RequirementFields {
  title_raw: string
  selected_ai: 'codex_cli' | 'claude_cli' | null
  goal_raw: string
  in_scope_raw: string
  not_in_scope_raw: string
  done_criteria_raw: string
}
export interface Draft {
  draft_id: string; revision: number; status: 'editable' | 'bound' | 'archived'
  fields: RequirementFields; content_hash: string; origin_task_id: string | null
}
export interface Binding { task_id: string; draft_id: string; revision: number; content_hash: string }
export interface RequirementMessage { id: string; seq: number; kind: string; body_raw: string | null; draft_id: string; revision: number }
interface Entry {
  project: string; origin: string | null; conversation: string; draft: Draft | null
  fields: RequirementFields; dirty: boolean; edits: number; composing: boolean
  saving: boolean; confirming: boolean; loading: boolean; error: string; message: string
  conflict: Draft | null; binding: Binding | null; messages: RequirementMessage[]; drafts: Draft[]
  saveRetry: { request: Record<string, unknown>; edits: number } | null
  confirmRetry: Record<string, unknown> | null
  messageInput: string; messagePending: boolean; messageRetry: Record<string, unknown> | null
}
const empty = (): RequirementFields => ({ title_raw: '', selected_ai: null, goal_raw: '', in_scope_raw: '', not_in_scope_raw: '', done_criteria_raw: '' })
const copy = <T>(v: T): T => JSON.parse(JSON.stringify(v))
function code(error: unknown) { return typeof error === 'object' && error && 'code' in error ? String(error.code) : 'REQUEST_FAILED' }
function request(project: string, payload: Record<string, unknown>, write = true): Record<string, unknown> {
  return { project_id: project, client_request_id: write ? crypto.randomUUID() : null, ...payload }
}
async function call<T>(request: Record<string, unknown>) {
  const { project_id, client_request_id, ...operation } = request
  return invoke<T>('requirement_workspace', { request: { project_id, client_request_id, operation } })
}

export const useWorkspaceStore = defineStore('workspace', {
  state: () => ({ entries: {} as Record<string, Entry>, activeKey: '', selection: 0, capability: 'blocked' }),
  getters: { current: state => state.entries[state.activeKey] ?? null },
  actions: {
    async open(project: string, origin: string | null = null) {
      const key = `${project}:${origin ?? 'initial'}`
      this.activeKey = key; const epoch = ++this.selection
      if (this.entries[key]) return
      const e: Entry = { project, origin, conversation: '', draft: null, fields: empty(), dirty: false, edits: 0,
        composing: false, saving: false, confirming: false, loading: true, error: '', message: '', conflict: null,
        binding: null, messages: [], drafts: [], saveRetry: null, confirmRetry: null,
        messageInput: '', messagePending: false, messageRetry: null }
      this.entries[key] = e
      // Always mutate the original entry, never "this.current" after await.
      const entry = this.entries[key]
      try {
        const opened = await call<{ conversation_id: string }>(request(project, { op: 'open' }))
        entry.conversation = opened.conversation_id
        const listed = await call<{ drafts: Draft[]; messages: RequirementMessage[] }>(request(project, { op: 'list', conversation_id: entry.conversation, after_seq: 0 }, false))
        entry.messages = listed.messages; entry.drafts = listed.drafts
        const candidates = listed.drafts.filter(d => d.origin_task_id === origin && d.status !== 'archived')
        const draft = candidates.find(d => d.status === 'editable') ?? candidates.at(-1) ?? await call<Draft>(request(project, { op: 'create', conversation_id: entry.conversation, origin_task_id: origin }))
        entry.draft = draft
        if (!entry.dirty && !entry.composing) entry.fields = copy(draft.fields)
        if (draft.status === 'bound') entry.binding = await call<Binding>(request(project, { op: 'binding', conversation_id: entry.conversation, draft_id: draft.draft_id }, false))
      } catch (error) { entry.error = code(error) }
      finally { entry.loading = false }
      // No current-selection writes here: late answers remain in their original cache.
      if (this.selection !== epoch) return
    },
    leave() { this.activeKey = ''; this.selection++ },
    edit(field: keyof RequirementFields, value: string) {
      const e = this.current
      if (!e || e.draft?.status !== 'editable' || e.confirming || e.confirmRetry) return
      if (field === 'selected_ai') {
        if (!['', 'codex_cli', 'claude_cli'].includes(value)) return
        e.fields.selected_ai = (value || null) as RequirementFields['selected_ai']
      } else e.fields[field] = value
      e.edits++; e.dirty = true; e.message = ''
    },
    async refresh(key?: string) {
      key ??= this.activeKey
      const e = this.entries[key]; if (!e?.draft) return
      const value = await call<Draft>(request(e.project, { op: 'get', conversation_id: e.conversation, draft_id: e.draft.draft_id }, false))
      if (e.dirty || e.composing || e.saveRetry) e.conflict = value
      else { e.draft = value; e.fields = copy(value.fields) }
    },
    async save(key?: string) {
      key ??= this.activeKey
      const e = this.entries[key]
      if (!e?.draft || e.loading || e.saving || e.confirming || e.confirmRetry || e.composing || e.conflict || (!e.dirty && !e.saveRetry)) return
      if (e.draft.status !== 'editable') return
      e.saving = true; e.error = ''; e.message = ''
      const pending = e.saveRetry ?? { request: request(e.project, { op: 'save', conversation_id: e.conversation,
        draft_id: e.draft.draft_id, expected_revision: e.draft.revision, fields: copy(e.fields) }), edits: e.edits }
      e.saveRetry = pending
      try {
        const value = await call<Draft>(pending.request)
        e.draft = value; e.saveRetry = null
        if (e.edits === pending.edits && !e.composing) { e.fields = copy(value.fields); e.dirty = false }
        e.message = e.dirty ? 'UNSAVED_CHANGES' : 'SAVED'
      } catch (error) {
        e.error = code(error)
        if (e.error === 'REVISION_CONFLICT') { e.saveRetry = null; await this.refresh(key).catch(() => {}) }
      } finally { e.saving = false }
    },
    // Explicit merge keeps local exact input; server version is shown separately.
    keepLocalAfterComparison() {
      const e = this.current
      if (!e?.conflict || e.conflict.status !== 'editable') return
      e.draft = e.conflict; e.conflict = null; e.saveRetry = null; e.confirmRetry = null; e.dirty = true; e.message = 'UNSAVED_CHANGES'
    },
    async confirm() {
      const e = this.current
      if (!e?.draft || e.dirty || e.saving || e.confirming || e.composing || e.conflict || e.saveRetry || e.draft.status !== 'editable') return
      if (!e.fields.selected_ai || Object.values(e.fields).some(value => !value || !value.trim())) { e.error = 'INCOMPLETE_SCOPE'; return }
      e.confirming = true; e.error = ''
      const pending = e.confirmRetry ?? request(e.project, { op: 'confirm', conversation_id: e.conversation,
        draft_id: e.draft.draft_id, expected_revision: e.draft.revision, content_hash: e.draft.content_hash, selected_ai: e.fields.selected_ai })
      e.confirmRetry = pending
      try {
        e.binding = await call<Binding>(pending); e.draft.status = 'bound'; e.confirmRetry = null; e.message = 'BOUND_NOT_STARTED'
      } catch (error) {
        e.error = code(error)
        // An unknown response must keep the same key/payload. Definitive rejections permit correction.
        if (e.error !== 'REQUEST_FAILED' && e.error !== 'DATABASE_ERROR') e.confirmRetry = null
      } finally { e.confirming = false }
    },
    async append() {
      const e = this.current
      if (!e?.draft || e.draft.status !== 'editable' || e.composing || e.messagePending || !e.messageInput) return
      const sent = e.messageInput
      const pending = e.messageRetry ?? request(e.project, { op: 'append', conversation_id: e.conversation,
        draft_id: e.draft.draft_id, expected_revision: e.draft.revision, body_raw: sent })
      e.messageRetry = pending; e.messagePending = true
      try {
        await call(pending); e.messageRetry = null
        if (e.messageInput === pending.body_raw) e.messageInput = ''
        const listed = await call<{ messages: RequirementMessage[] }>(request(e.project, { op: 'list', conversation_id: e.conversation, after_seq: 0 }, false))
        e.messages = listed.messages
      } catch (error) { e.error = code(error); if (e.error === 'REVISION_CONFLICT') e.messageRetry = null }
      finally { e.messagePending = false }
    },
  },
})
