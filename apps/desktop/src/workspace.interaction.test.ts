import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { defineComponent, h, ref } from 'vue'
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
import { invoke } from '@tauri-apps/api/core'
import { useWorkspaceStore, type Draft, type RequirementFields } from './stores/workspace'
import RequirementWorkspace from './components/RequirementWorkspace.vue'
import { all, button, click, renderComponent, settle, type TestNode } from './testSupport/interactionRenderer'
const mocked = vi.mocked(invoke)
const empty = (): RequirementFields => ({ title_raw: '', selected_ai: null, goal_raw: '', in_scope_raw: '', not_in_scope_raw: '', done_criteria_raw: '' })
const fields = (): RequirementFields => ({ title_raw: ' title ', selected_ai: 'codex_cli', goal_raw: '  same\r\nsame\r\n', in_scope_raw: 'one\none', not_in_scope_raw: 'no AI', done_criteria_raw: 'check\ncheck' })
const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v))
function deferred<T>() { let resolve!: (v: T) => void; let reject!: (e: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b }); return { promise, resolve, reject } }
let drafts: Record<string, Draft>
let calls: any[]
let handler: ((r: any) => Promise<any> | undefined) | undefined
let mounted: ReturnType<typeof renderComponent> | undefined
function route() {
  mocked.mockImplementation(async (command, args: any) => {
    if (command !== 'requirement_workspace') throw new Error(`Unexpected IPC ${command}`)
    const r = { ...args.request.operation, project_id: args.request.project_id, client_request_id: args.request.client_request_id }
    calls.push(clone(r))
    const handled = handler?.(r)
    if (handled) return handled
    const key = `${r.project_id}:${r.origin_task_id ?? 'initial'}`
    if (r.op === 'open') return { conversation_id: `c-${r.project_id}` } as any
    if (r.op === 'list') return { drafts: Object.values(drafts).filter(d => d.draft_id.startsWith(`${r.project_id}:`)), messages: [] } as any
    if (r.op === 'create') {
      drafts[key] ??= { draft_id: key, revision: 1, status: 'editable', content_hash: 'hash1', origin_task_id: r.origin_task_id, fields: empty() }
      return clone(drafts[key]) as any
    }
    const d = drafts[r.draft_id]
    if (r.op === 'get') return clone(d) as any
    if (r.op === 'binding') return { task_id: 'bound-task', draft_id: r.draft_id, revision: d.revision } as any
    if (r.op === 'save') {
      if (d.revision !== r.expected_revision) throw { code: 'REVISION_CONFLICT' }
      Object.assign(d, { revision: d.revision + 1, content_hash: `hash${d.revision + 1}`, fields: clone(r.fields) })
      return clone(d) as any
    }
    if (r.op === 'confirm') { d.status = 'bound'; return { task_id: 'bound-task', draft_id: d.draft_id, revision: d.revision } as any }
    if (r.op === 'append') return { saved: true } as any
    throw new Error(`Unexpected operation ${r.op}`)
  })
}
function editor(root: TestNode, field = 'goal_raw') { return all(root, n => n.props['data-field'] === field)[0] }
async function type(node: TestNode, value: string) { node.value = value; await node.props.onInput({ target: node }); await settle() }
beforeEach(() => {
  setActivePinia(createPinia()); drafts = {}; calls = []; handler = undefined; mocked.mockReset(); route()
  vi.stubGlobal('document', { activeElement: null }); vi.stubGlobal('Document', class {}); vi.stubGlobal('ShadowRoot', class {})
})
afterEach(() => { mounted?.unmount(); mounted = undefined; vi.useRealTimers(); vi.unstubAllGlobals() })

describe('non-AI workspace', () => {
  it('late project-open data cannot replace the selected project or unsaved buffer', async () => {
    const s = useWorkspaceStore(); const wait = deferred<any>()
    handler = r => r.op === 'open' && r.project_id === 'p' ? wait.promise : undefined
    const opening = s.open('p'); await s.open('q'); s.edit('goal_raw', 'current')
    wait.resolve({ conversation_id: 'c-p' }); await opening
    expect(s.current!.project).toBe('q'); expect(s.current!.fields.goal_raw).toBe('current')
    expect(s.current!.loading).toBe(false); expect(s.current!.error).toBe('')
  })
  it('late autosave from the old project does not cancel the new project autosave', async () => {
    vi.useFakeTimers(); const project = ref('p'); const pinia = createPinia(); setActivePinia(pinia)
    mounted = renderComponent(defineComponent(() => () => h(RequirementWorkspace, { projectId: project.value })), pinia); await settle()
    const s = useWorkspaceStore(); const wait = deferred<Draft>(); let first = true
    handler = r => r.op === 'save' && r.project_id === 'p' && first ? (first = false, wait.promise) : undefined
    await type(editor(mounted.root), 'p-first'); await vi.advanceTimersByTimeAsync(500)
    await type(editor(mounted.root), 'p-newer'); project.value = 'q'; await settle()
    await type(editor(mounted.root), 'q-text')
    drafts['p:initial'].revision = 2
    wait.resolve({ ...clone(drafts['p:initial']), fields: { ...empty(), goal_raw: 'p-first' } }); await settle()
    await vi.advanceTimersByTimeAsync(1200); await settle()
    expect(s.current!.project).toBe('q'); expect(drafts['q:initial'].fields.goal_raw).toBe('q-text')
    expect(s.entries['p:initial'].fields.goal_raw).toBe('p-newer'); expect(s.entries['p:initial'].dirty).toBe(false)
  })
  it('uncertain confirmation retries its original request and cannot edit into a duplicate task', async () => {
    const s = useWorkspaceStore(); await s.open('p'); Object.assign(s.current!.fields, fields()); s.current!.dirty = true; s.current!.edits++; await s.save()
    handler = r => r.op === 'confirm' ? Promise.reject(new Error('response lost')) : undefined
    await s.confirm(); s.edit('goal_raw', 'must not alter uncertain request'); await s.save()
    expect(s.current!.fields.goal_raw).toBe(fields().goal_raw)
    const pending = deferred<any>(); handler = r => r.op === 'confirm' ? pending.promise : undefined
    const retry = s.confirm(); await s.open('q'); s.edit('goal_raw', 'other project')
    pending.resolve({ task_id: 'already-created', draft_id: 'p:initial', revision: 2 }); await retry
    const confirmations = calls.filter(c => c.op === 'confirm'); expect(confirmations[0]).toEqual(confirmations[1])
    expect(s.current!.binding).toBeNull(); expect(s.current!.fields.goal_raw).toBe('other project')
    await s.open('p'); expect(s.current!.binding!.task_id).toBe('already-created')
  })
  it('incomplete confirmation explains the reason without IPC or AI', async () => {
    const s = useWorkspaceStore(); await s.open('p'); await s.confirm()
    expect(s.current!.error).toBe('INCOMPLETE_SCOPE'); expect(calls.filter(c => c.op === 'confirm')).toHaveLength(0)
  })
  it('language labels do not rewrite raw fields or revision hashes', async () => {
    const pinia = createPinia(); setActivePinia(pinia); const s = useWorkspaceStore(); await s.open('p')
    Object.assign(s.current!.fields, fields()); s.current!.dirty = true; s.current!.edits++; await s.save()
    mounted = renderComponent(defineComponent(() => () => h(RequirementWorkspace, { projectId: 'p' })), pinia); await settle()
    const before = clone(s.current!.draft!)
    const checkbox = all(mounted.root, n => n.kind === 'input' && n.props.type === 'checkbox')[0]
    checkbox.checked = true; checkbox.dispatchEvent(new Event('change')); await settle()
    expect(mounted.root.textContent).toContain('Goal'); expect(s.current!.fields).toEqual(before.fields); expect(s.current!.draft!.content_hash).toBe(before.content_hash)
    expect(calls.filter(c => c.op === 'save')).toHaveLength(1)
  })
  it('restores exact saved text after store recreation and does not call AI for saves or reads', async () => {
    const s = useWorkspaceStore(); await s.open('p'); Object.assign(s.current!.fields, fields()); s.current!.dirty = true; s.current!.edits++
    await s.save(); expect(s.current!.draft!.fields).toEqual(fields())
    setActivePinia(createPinia()); const restored = useWorkspaceStore(); await restored.open('p')
    expect(restored.current!.fields).toEqual(fields())
    expect(calls.every(c => ['open', 'list', 'create', 'save'].includes(c.op))).toBe(true)
    expect(calls.some(c => /plan|start|test|build|accept/.test(c.op))).toBe(false)
  })
  it('keeps newer input when a save returns late, then saves against the returned revision', async () => {
    const s = useWorkspaceStore(); await s.open('p'); s.edit('goal_raw', 'first')
    const wait = deferred<Draft>(); handler = r => r.op === 'save' ? wait.promise : undefined
    const saving = s.save(); s.edit('goal_raw', 'newer\nnewer')
    wait.resolve({ ...clone(s.current!.draft!), revision: 2, fields: { ...empty(), goal_raw: 'first' } }); await saving
    expect(s.current!.fields.goal_raw).toBe('newer\nnewer'); expect(s.current!.dirty).toBe(true); expect(s.current!.draft!.revision).toBe(2)
  })
  it('retries a lost save response with the same key and exact payload, without losing later edits', async () => {
    const s = useWorkspaceStore(); await s.open('p'); s.edit('goal_raw', 'first')
    handler = r => r.op === 'save' ? Promise.reject(new Error('transport')) : undefined
    await s.save(); s.edit('goal_raw', 'second')
    handler = r => r.op === 'save' ? Promise.resolve({ ...clone(s.current!.draft!), revision: 2, fields: r.fields }) : undefined
    await s.save(); const saves = calls.filter(c => c.op === 'save'); expect(saves[0]).toEqual(saves[1]); expect(s.current!.fields.goal_raw).toBe('second'); expect(s.current!.dirty).toBe(true)
  })
  it('shows conflict separately and requires explicit compare before preserving local input on new head', async () => {
    const s = useWorkspaceStore(); await s.open('p'); s.edit('goal_raw', 'local')
    drafts['p:initial'].revision = 2; drafts['p:initial'].fields.goal_raw = 'remote'
    await s.save(); expect(s.current!.error).toBe('REVISION_CONFLICT'); expect(s.current!.fields.goal_raw).toBe('local'); expect(s.current!.conflict!.fields.goal_raw).toBe('remote')
    await s.confirm(); expect(calls.filter(c => c.op === 'confirm')).toHaveLength(0)
    s.keepLocalAfterComparison(); await s.save(); expect(drafts['p:initial'].fields.goal_raw).toBe('local'); expect(drafts['p:initial'].revision).toBe(3)
  })
  it('binds only once during duplicate clicks and retains binding when execution has not started', async () => {
    const s = useWorkspaceStore(); await s.open('p'); Object.assign(s.current!.fields, fields()); s.current!.dirty = true; s.current!.edits++; await s.save(); const wait = deferred<any>(); handler = r => r.op === 'confirm' ? wait.promise : undefined
    const pending = s.confirm(); await s.confirm(); expect(calls.filter(c => c.op === 'confirm')).toHaveLength(1)
    wait.resolve({ task_id: 't', draft_id: 'p:initial', revision: 1 }); await pending
    expect(s.current!.binding!.task_id).toBe('t'); expect(s.current!.draft!.status).toBe('bound')
    expect(calls.filter(c => /start|verify|accept/.test(c.op))).toHaveLength(0)
  })
  it('does not overwrite another project or its busy/error status with late success or failure', async () => {
    const s = useWorkspaceStore(); await s.open('p'); s.edit('goal_raw', 'old-project')
    const wait = deferred<Draft>(); handler = r => r.op === 'save' ? wait.promise : undefined
    const pending = s.save(); await s.open('q'); s.edit('goal_raw', 'new-project')
    wait.reject({ code: 'DATABASE_ERROR' }); await pending
    expect(s.current!.project).toBe('q'); expect(s.current!.fields.goal_raw).toBe('new-project'); expect(s.current!.error).toBe(''); expect(s.current!.saving).toBe(false)
    await s.open('p'); expect(s.current!.fields.goal_raw).toBe('old-project'); expect(s.current!.error).toBe('DATABASE_ERROR')
  })
  it('keeps next-round draft independent from existing project draft and sends exact origin', async () => {
    const s = useWorkspaceStore(); await s.open('p'); s.edit('goal_raw', 'initial'); await s.save(); await s.open('p', 'old-task'); s.edit('goal_raw', 'next')
    await s.save(); expect(s.current!.origin).toBe('old-task'); await s.open('p'); expect(s.current!.fields.goal_raw).toBe('initial')
    expect(calls.find(c => c.op === 'create' && c.origin_task_id === 'old-task')).toBeTruthy()
  })
  it('IME blocks autosave until composition end; interface exposes blocked planning and saved revision', async () => {
    vi.useFakeTimers(); const project = ref('p'); const component = defineComponent(() => () => h(RequirementWorkspace, { projectId: project.value, taskId: 'task' }))
    mounted = renderComponent(component, createPinia()); await settle(); const s = useWorkspaceStore()
    const node = editor(mounted.root); node.props.onCompositionstart(new Event('compositionstart')); await type(node, '拼音中')
    await vi.advanceTimersByTimeAsync(800); expect(calls.filter(c => c.op === 'save')).toHaveLength(0)
    node.props.onCompositionend({ target: node }); await vi.advanceTimersByTimeAsync(700); await settle()
    expect(calls.filter(c => c.op === 'save')).toHaveLength(1); expect(s.current!.fields.goal_raw).toBe('拼音中')
    expect(button(mounted.root, 'AI 整理（尚未开放）').props.disabled).toBe(true)
    expect(mounted.root.textContent).toContain('已保存版本 2')
    project.value = 'q'; await settle(); expect(editor(mounted.root).value).toBe('')
  })
  it('UI input survives save failure and pending confirmation is visibly disabled', async () => {
    const pinia = createPinia(); setActivePinia(pinia); mounted = renderComponent(defineComponent(() => () => h(RequirementWorkspace, { projectId: 'p' })), pinia); await settle()
    const s = useWorkspaceStore(); await type(editor(mounted.root), 'preserve')
    handler = r => r.op === 'save' ? Promise.reject({ code: 'DATABASE_ERROR' }) : undefined
    await click(button(mounted.root, '保存草稿')); expect(editor(mounted.root).value).toBe('preserve'); expect(mounted.root.textContent).toContain('输入已保留')
    handler = undefined; await click(button(mounted.root, '保存草稿')); const wait = deferred<any>(); handler = r => r.op === 'confirm' ? wait.promise : undefined
    Object.assign(s.current!.fields, fields()); s.current!.dirty = true; s.current!.edits++; handler = undefined; await s.save(); handler = r => r.op === 'confirm' ? wait.promise : undefined
    const pending = s.confirm(); await settle(); expect(button(mounted.root, '正在确认…').props.disabled).toBe(true)
    wait.resolve({ task_id: 't', draft_id: 'p:initial', revision: 2 }); await pending; await settle(); expect(mounted.root.textContent).toContain('未自动启动')
  })
})
