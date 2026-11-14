import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))
import { invoke } from '@tauri-apps/api/core'
import App from './App.vue'
import Workspace from './components/readonly/ReadonlyWorkspace.vue'
import { useWorkspaceStore, type Draft } from './stores/workspace'
import { useReadonlyWorkspace } from './stores/readonlyWorkspace'
import { all, click, renderComponent, settle, type TestNode } from './testSupport/interactionRenderer'
const ipc = vi.mocked(invoke), clone = <T>(v: T): T => JSON.parse(JSON.stringify(v))
let mounted: ReturnType<typeof renderComponent> | undefined
let calls: any[], drafts: Record<string, Draft>, intercept: ((r: any) => Promise<any> | undefined) | undefined
function node(attr: string, value?: string) { const found = all(mounted!.root, n => value === undefined ? n.props[attr] !== undefined : n.props[attr] === value); expect(found).toHaveLength(1); return found[0] }
const ops = () => calls.map(c => c.op)
async function edit(text: string) { const n = node('data-formal-field', 'goal_raw'); n.value = text; await n.props.onInput({ target: n }); await settle(); return n }
async function mount() { mounted = renderComponent(Workspace, createPinia()); await settle(); await click(node('data-project', 'p')); await click(node('data-task', 'a')) }
async function open(next = false) { await click(node('data-formal-entry')); expect(calls).toHaveLength(0); await click(node(next ? 'data-open-next' : 'data-open-project')) }
beforeEach(() => {
  vi.useFakeTimers(); setActivePinia(createPinia()); ipc.mockReset(); calls = []; drafts = {}; intercept = undefined
  const cache = new Map<string, string>(); vi.stubGlobal('sessionStorage', { getItem: (k: string) => cache.get(k) ?? null, setItem: (k: string, v: string) => cache.set(k, v) })
  ipc.mockImplementation(async (cmd, args: any) => {
    if (cmd === 'initialize_app') return undefined
    if (cmd === 'get_provider' || cmd === 'get_setting') return null as any
    if (cmd === 'list_conversations') return [] as any
    if (cmd === 'list_projects') return [{ id: 'p', name: 'Fixture', local_path: 'fixture' }, { id: 'q', name: 'Other', local_path: 'other' }] as any
    if (cmd === 'list_tasks_for_project') return ['a','b'].map(id => ({ id, project_id: args.projectId, title: id, state: 'draft' })) as any
    if (cmd === 'get_task_evidence_overview') return { timeline: [], commands: [], tests: [], builds: [], file_changes: [], git_events: [], acceptance_history: [] } as any
    if (cmd !== 'requirement_workspace') throw Error('Forbidden IPC: ' + cmd)
    const r = { ...args.request.operation, project: args.request.project_id, key: args.request.client_request_id }; calls.push(clone(r))
    const response = intercept?.(r); if (response) return response
    if (r.op === 'open') return { conversation_id: r.project } as any
    if (r.op === 'list') return { drafts: Object.values(drafts).filter(d => d.draft_id.startsWith(r.project + ':')), messages: [] } as any
    if (r.op === 'create') { const id = r.project + ':' + (r.origin_task_id ?? 'initial'); drafts[id] ??= { draft_id: id, revision: 1, status: 'editable', content_hash: 'h', origin_task_id: r.origin_task_id, fields: { title_raw: '', selected_ai: null, goal_raw: '', in_scope_raw: '', not_in_scope_raw: '', done_criteria_raw: '' } }; return clone(drafts[id]) as any }
    if (r.op === 'get') return clone(drafts[r.draft_id]) as any
    if (r.op === 'binding') return { task_id: 'existing', draft_id: r.draft_id } as any
    if (r.op === 'save') { const d = drafts[r.draft_id]; if (d.revision !== r.expected_revision) throw { code: 'REVISION_CONFLICT' }; d.fields = clone(r.fields); d.revision++; return clone(d) as any }
    throw Error('Forbidden workspace op: ' + r.op)
  })
})
afterEach(() => { mounted?.unmount(); mounted = undefined; expect(ops().every(op => ['open','list','create','get','save','binding'].includes(op))).toBe(true); vi.useRealTimers(); vi.unstubAllGlobals() })
describe('manual persistent draft actual preview integration', () => {
  it('requires explicit open, keeps preview cache separate and saves incomplete exact text only on click', async () => {
    await mount(); const ro = useReadonlyWorkspace(); ro.editDraft('window\nonly'); await open(); expect(ops()).toEqual(['open','list','create'])
    const n = await edit('  ÄZ\r\n原文\n'); n.props.onCompositionstart(); await settle(); expect(node('data-save').props.disabled).toBe(true)
    n.props.onCompositionend({ target: n }); await settle(); n.dispatchEvent(new Event('blur')); n.dispatchEvent(new Event('keydown')); await vi.advanceTimersByTimeAsync(5000)
    expect(ops()).not.toContain('save'); await click(node('data-language')); expect(mounted!.root.textContent).toContain('Persistent draft')
    await click(node('data-save')); expect(ops().filter(v => v === 'save')).toHaveLength(1); expect(drafts['p:initial'].fields.goal_raw).toBe('  ÄZ\r\n原文\n'); expect(drafts['p:initial'].fields.selected_ai).toBe(null)
    await click(node('data-draft-back')); expect(ro.draft).toBe('window\nonly'); await click(node('data-task','b')); expect(calls.filter(c => c.op === 'open')).toHaveLength(1)
  })
  it('protects dirty/IME leave without saving, reopens cached input, and isolates project/task origins', async () => {
    await mount(); await open(true); await edit('next\n'); await click(node('data-draft-back')); expect(node('data-leave-prompt')).toBeTruthy()
    await click(node('data-leave-stay')); await click(node('data-draft-back')); await click(node('data-leave-keep')); expect(ops()).not.toContain('save')
    await click(node('data-formal-entry')); await click(node('data-open-next')); expect(node('data-formal-field','goal_raw').value).toBe('next\n')
    await click(node('data-draft-back')); await click(node('data-leave-keep')); await click(node('data-project','q')); await click(node('data-formal-entry')); await click(node('data-open-project'))
    expect(useWorkspaceStore().current?.project).toBe('q'); expect(useWorkspaceStore().entries['p:a'].fields.goal_raw).toBe('next\n'); expect(calls.find(c => c.op === 'create').origin_task_id).toBe('a')
  })
  it('locks scope while saving and retries an unknown response with original key/payload, keeping newer input', async () => {
    await mount(); await open(); await edit('first'); let reject!: (x: any) => void
    intercept = r => r.op === 'save' ? new Promise((_a,b) => { reject = b }) : undefined
    const pending = click(node('data-save')); await settle(); expect(node('data-draft-back').props.disabled).toBe(true); expect(all(mounted!.root,n => n.props['data-project'])).toHaveLength(0)
    await edit('newer'); reject({ code: 'REQUEST_FAILED' }); await pending; expect(mounted!.root.textContent).toContain('保存未确认'); expect(node('data-refresh-draft').props.disabled).toBe(true)
    const original = calls.find(c => c.op === 'save'); intercept = undefined; await click(node('data-save')); expect(calls.filter(c => c.op === 'save')[1]).toEqual(original)
    expect(node('data-formal-field','goal_raw').value).toBe('newer'); expect(mounted!.root.textContent).toContain('未保存'); await click(node('data-save')); expect(drafts['p:initial'].fields.goal_raw).toBe('newer')
  })
  it('shows server conflict without merging and requires compare then explicit save', async () => {
    await mount(); await open(); await edit('local\n'); drafts['p:initial'].revision = 2; drafts['p:initial'].fields.goal_raw = 'server'
    await click(node('data-save')); expect(node('data-conflict').textContent).toContain('server'); expect(node('data-formal-field','goal_raw').value).toBe('local\n')
    expect(node('data-save').props.disabled).toBe(true); const count = calls.length; await click(node('data-keep-local')); expect(calls).toHaveLength(count); await click(node('data-save')); expect(drafts['p:initial'].revision).toBe(3)
    await click(node('data-refresh-draft')); expect(ops().at(-1)).toBe('get')
  })
  it('restores existing/bound drafts without create and provides no confirm or replacement action', async () => {
    await mount(); await open(); await edit('saved'); await click(node('data-save')); await click(node('data-draft-back'))
    useWorkspaceStore().$reset(); drafts['p:initial'].status = 'bound'; calls = []; await open()
    expect(ops()).toEqual(['open','list','binding']); expect(node('data-formal-field','goal_raw').props.readonly).toBe(true); expect(node('data-save').props.disabled).toBe(true); expect(mounted!.root.textContent).toContain('existing')
  })
  it('shows failed opening without success, retries only explicitly, and never saves on return or unmount', async () => {
    await mount(); intercept = r => r.op === 'open' ? Promise.reject(Error('private')) : undefined; await open(); expect(mounted!.root.textContent).not.toContain('已保存版本'); expect(mounted!.root.textContent).not.toContain('private')
    await vi.advanceTimersByTimeAsync(5000); expect(ops()).toEqual(['open']); intercept = undefined; await click(node('data-retry-open')); await edit('retain')
    await click(node('data-return')); await click(node('data-leave-stay')); expect(node('data-formal-field','goal_raw').value).toBe('retain')
    mounted!.unmount(); mounted = undefined; expect(ops()).not.toContain('save')
  })
})

// Exercise the actual production App v-if/close path, not an artificial parent unmount.
async function appPreview() {
  mounted = renderComponent(App, createPinia()); await settle()
  await click(node('data-readonly-entry')); await vi.dynamicImportSettled(); await settle()
  await click(node('data-project', 'p')); await click(node('data-task', 'a'))
}
async function reenterAppDraft() {
  await click(node('data-readonly-entry')); await vi.dynamicImportSettled(); await settle()
  await click(node('data-project', 'p')); await click(node('data-task', 'a'))
  await click(node('data-formal-entry')); await click(node('data-open-project'))
}
describe('actual App leave path and request retention', () => {
  it('guards parent removal for dirty input, retains it through App return/reentry, and never autosaves', async () => {
    await appPreview(); await open(); await edit('  keep\nexact\n')
    await click(node('data-return')); expect(node('data-leave-prompt')).toBeTruthy()
    await click(node('data-leave-stay')); expect(node('data-draft-editor')).toBeTruthy()
    await click(node('data-return')); await click(node('data-leave-keep'))
    expect(all(mounted!.root,n=>n.props['data-draft-editor']!==undefined)).toHaveLength(0)
    expect(useWorkspaceStore().entries['p:initial'].fields.goal_raw).toBe('  keep\nexact\n')
    await reenterAppDraft(); expect(node('data-formal-field','goal_raw').value).toBe('  keep\nexact\n')
    expect(ops()).not.toContain('save'); mounted!.unmount(); mounted=undefined; expect(ops()).not.toContain('save')
  })
  it('requires a choice for IME even without dirty fields on both controlled return paths', async () => {
    await appPreview(); await open(); node('data-formal-field','goal_raw').props.onCompositionstart(); await settle()
    expect(useWorkspaceStore().current?.dirty).toBe(false)
    await click(node('data-return')); expect(node('data-leave-prompt')).toBeTruthy(); await click(node('data-leave-stay'))
    await click(node('data-draft-back')); expect(node('data-leave-prompt')).toBeTruthy(); await click(node('data-leave-keep'))
    expect(ops()).not.toContain('save'); await click(node('data-task','b')); expect(useWorkspaceStore().entries['p:initial']).toBeTruthy()
  })
  it('blocks App parent removal while saving; unknown payload/key survive explicit leave and reentry', async () => {
    await appPreview(); await open(); await edit('original\n')
    let reject!: (reason: any)=>void
    intercept=r=>r.op==='save'?new Promise((_resolve,b)=>{reject=b}):undefined
    const saving=click(node('data-save')); await settle()
    const entry=useWorkspaceStore().entries['p:initial']; const original=clone(entry.saveRetry)
    await click(node('data-return')); expect(node('data-draft-editor')).toBeTruthy(); expect(node('data-draft-back').props.disabled).toBe(true)
    expect(entry.saveRetry).toEqual(original); expect(all(mounted!.root,n=>n.props['data-leave-prompt']!==undefined)).toHaveLength(0)
    reject({code:'REQUEST_FAILED'}); await saving
    await click(node('data-return')); expect(node('data-leave-prompt')).toBeTruthy(); await click(node('data-leave-keep'))
    expect(entry.saveRetry).toEqual(original); expect(useWorkspaceStore().activeKey).toBe('')
    await reenterAppDraft(); expect(useWorkspaceStore().current?.saveRetry).toEqual(original)
    const savesBefore=ops().filter(op=>op==='save').length; expect(savesBefore).toBe(1)
    intercept=undefined; await click(node('data-save')); const saves=calls.filter(c=>c.op==='save'); expect(saves[1]).toEqual(saves[0])
    expect(node('data-reload-warning').textContent).toContain('刷新或关闭可能丢失')
  })
})
