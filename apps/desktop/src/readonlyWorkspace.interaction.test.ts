import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import ReadonlyWorkspace from './components/readonly/ReadonlyWorkspace.vue'
import App from './App.vue'
import { useReadonlyWorkspace } from './stores/readonlyWorkspace'
import type { Project, Task, TaskEvidenceOverview } from './types'
import { all, button, click, renderComponent, settle, type TestNode } from './testSupport/interactionRenderer'
const ipc = vi.mocked(invoke), events = vi.mocked(listen), stop = vi.fn()
let mounted: ReturnType<typeof renderComponent> | undefined
let handler: (event: any) => void
const project = { id: 'p', name: 'Product records', local_path: 'C:/isolated-product' } as Project
const task = (id: string) => ({ id, project_id: 'p', title: id === 'a' ? 'Recorded task A' : 'Task B', state: 'running', result_generation: 2 }) as Task
function overview(id = 'a'): TaskEvidenceOverview {
  return { project_id: 'p', task_id: id, task_state: 'running', result_generation: 2, pending_round: 1, needs_user_action: false,
    acceptance_ready: false, is_completed: false, file_snapshot_available: false, current_file_changes: [], file_changes: [], git_events: [], acceptance_history: [],
    timeline: [{ id: 'event-a', event_type: 'test', title: '<img src=x onerror=alert(1)>', status: 'future_status', started_at: '2026-11-14T04:00:00Z', finished_at: null, result_generation: 1, round: 1, is_historical: true, needs_user_action: false }],
    commands: [{ id: 'cmd', project_id: 'p', task_id: id, command_redacted: 'npm test', exit_code: 0, started_at: null, finished_at: null, stdout_ref: 'Authorization: Bearer fake-private-value', stderr_ref: null, created_at: '' }],
    tests: [{ id: 'test', project_id: 'p', task_id: id, result_generation: 1, round: 1, test_command_redacted: 'npm test', status: 'passed', display_status: 'historical', passed_count: 1, failed_count: 0, skipped_count: 0, exit_code: 0, reason: null, stdout_ref: null, stderr_ref: null, created_at: '', is_current_generation: false, is_current_attempt: false, is_adopted: false }], builds: [] }
}
function node(root: TestNode, attr: string, value?: string) { const found = all(root, n => value === undefined ? n.props[attr] !== undefined : n.props[attr] === value); expect(found).toHaveLength(1); return found[0] }
async function type(text: string) { const editor = all(mounted!.root, n => n.kind === 'textarea')[0]; editor.value = text; Object.assign(editor, { style: {}, scrollHeight: 80 }); await editor.props.onInput({ target: editor }); await settle(); return editor }
beforeEach(() => {
  setActivePinia(createPinia()); vi.useFakeTimers(); stop.mockReset(); ipc.mockReset(); events.mockReset()
  const cache = new Map<string, string>(); vi.stubGlobal('sessionStorage', { getItem: (k: string) => cache.get(k) ?? null, setItem: (k: string, v: string) => cache.set(k, v) })
  events.mockImplementation(async (_name, callback) => { handler = callback; return stop })
  ipc.mockImplementation(async (cmd, args: any) => {
    if (cmd === 'list_projects') return [project] as any
    if (cmd === 'list_tasks_for_project') return [task('a'), task('b')] as any
    if (cmd === 'get_task_evidence_overview') return overview(args.taskId) as any
    if (cmd === 'initialize_app') return undefined
    if (cmd === 'get_provider' || cmd === 'get_setting') return null as any
    if (cmd === 'list_conversations') return [] as any
    throw new Error('Forbidden IPC: ' + cmd)
  })
})
afterEach(() => { mounted?.unmount(); mounted = undefined; vi.useRealTimers(); vi.unstubAllGlobals() })
async function selected() {
  mounted = renderComponent(ReadonlyWorkspace, createPinia()); await settle()
  await click(node(mounted.root, 'data-project', 'p')); await click(node(mounted.root, 'data-task', 'a')); await settle()
}
function assertReadOnly() { expect(ipc.mock.calls.every(([name]) => ['list_projects', 'list_tasks_for_project', 'get_task_evidence_overview'].includes(name))).toBe(true) }
describe('actual read-only preview components', () => {
  it('keeps App legacy default and mounts preview only on opt-in; returning removes it', async () => {
    mounted = renderComponent(App, createPinia()); await settle()
    expect(all(mounted.root, n => n.props['data-return'] !== undefined)).toHaveLength(0)
    ipc.mockClear(); await click(node(mounted.root, 'data-readonly-entry')); await vi.dynamicImportSettled(); await settle()
    expect(mounted.root.textContent).toContain('任务只读工作台'); assertReadOnly()
    await click(node(mounted.root, 'data-return')); expect(all(mounted.root, n => n.props['data-return'] !== undefined)).toHaveLength(0)
    expect(stop).toHaveBeenCalledTimes(1)
  })
  it('selects real DTO records, distinguishes history/unknown status and text-only evidence', async () => {
    await selected(); expect(mounted!.root.textContent).toContain('记录阶段'); expect(mounted!.root.textContent).toContain('future_status')
    expect(mounted!.root.textContent).toContain('历史记录'); expect(all(mounted!.root, n => n.kind === 'img')).toHaveLength(0)
    expect(all(mounted!.root, n => n.props['data-evidence'] !== undefined)).toHaveLength(0)
    await click(node(mounted!.root, 'data-evidence-toggle'))
    expect(mounted!.root.textContent).toContain('未标记为已采用'); expect(mounted!.root.textContent).toContain('当前文件快照未知')
    expect(mounted!.root.textContent).toContain('[REDACTED]'); expect(mounted!.root.textContent).not.toContain('fake-private-value')
    assertReadOnly()
  })
  it('typing, Enter, IME, language/theme and returning cannot send or lose exact drafts', async () => {
    await selected(); const text = '  ÄZ\n草稿\n'; const editor = await type(text)
    editor.dispatchEvent(new Event('compositionstart')); editor.dispatchEvent(new Event('compositionend'))
    const root = all(mounted!.root, n => n.props.lang === 'zh-CN')[0]; root.props.onKeydown({ key: 'Enter', isComposing: true })
    await click(node(mounted!.root, 'data-language')); expect(useReadonlyWorkspace().draft).toBe(text)
    expect(mounted!.root.textContent).toContain('Window draft, not sent or confirmed')
    await click(button(mounted!.root, 'Warm paper')); await click(node(mounted!.root, 'data-task', 'b')); await type('other')
    await click(node(mounted!.root, 'data-task', 'a')); expect(useReadonlyWorkspace().draft).toBe(text)
    expect(all(mounted!.root, n => n.kind === 'button' && n.props.disabled).map(n => n.textContent)).toContain('Sending and execution unavailable')
    mounted!.unmount(); mounted = renderComponent(ReadonlyWorkspace, createPinia()); await settle()
    await click(node(mounted.root, 'data-project', 'p')); await click(node(mounted.root, 'data-task', 'a'))
    expect(useReadonlyWorkspace().draft).toBe(text); assertReadOnly()
  })
  it('polls serially, filters done events and cleans timer/listener and late subscriptions', async () => {
    await selected(); const before = ipc.mock.calls.length
    handler({ payload: { task_id: 'other' } }); await settle(); expect(ipc.mock.calls).toHaveLength(before)
    handler({ payload: { task_id: 'a' } }); await settle(); expect(ipc.mock.calls).toHaveLength(before + 1)
    await vi.advanceTimersByTimeAsync(3000); expect(ipc.mock.calls).toHaveLength(before + 2)
    mounted!.unmount(); mounted = undefined; const final = ipc.mock.calls.length
    await vi.advanceTimersByTimeAsync(9000); handler({ payload: { task_id: 'a' } }); await settle()
    expect(ipc.mock.calls).toHaveLength(final); expect(stop).toHaveBeenCalledTimes(1)
    let ready!: (fn: () => void) => void; events.mockImplementationOnce(() => new Promise(resolve => { ready = resolve }))
    mounted = renderComponent(ReadonlyWorkspace, createPinia()); await settle(); mounted.unmount(); mounted = undefined
    ready(stop); await settle(); expect(stop).toHaveBeenCalledTimes(2)
  })
  it('exposes refresh errors and stale last-read time without inventing task failure', async () => {
    await selected(); ipc.mockRejectedValueOnce(new Error('private exception')); await click(button(mounted!.root, '刷新记录'))
    expect(mounted!.root.textContent).toContain('上次快照已过期'); expect(mounted!.root.textContent).toContain('上次成功读取')
    expect(mounted!.root.textContent).not.toContain('private exception'); expect(useReadonlyWorkspace().overview?.task_state).toBe('running')
    await click(button(mounted!.root, '刷新记录')); expect(mounted!.root.textContent).not.toContain('上次快照已过期'); assertReadOnly()
  })
  it('shows empty project and task states instead of seeded history', async () => {
    ipc.mockResolvedValueOnce([]); mounted = renderComponent(ReadonlyWorkspace, createPinia()); await settle()
    expect(mounted.root.textContent).toContain('暂无已登记项目'); expect(all(mounted.root, n => n.props['data-timeline'])).toHaveLength(0)
    ipc.mockResolvedValueOnce([project]); await useReadonlyWorkspace().loadProjects(); ipc.mockResolvedValueOnce([])
    await click(node(mounted.root, 'data-project', 'p')); expect(mounted.root.textContent).toContain('此项目暂无任务记录'); assertReadOnly()
  })
})
