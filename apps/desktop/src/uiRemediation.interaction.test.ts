import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { nextTick } from 'vue'
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import App from './App.vue'
import CoreLoopView from './components/CoreLoopView.vue'
import { useNovaStore } from './stores/nova'
import { useCoreLoopStore } from './stores/coreLoop'
import type { Project, Task, TaskEvidenceOverview, TestEvidenceEntry, BuildEvidenceEntry } from './types'
import { all, button, click, input, renderComponent, settle, type TestNode } from './testSupport/interactionRenderer'

const mockedInvoke = vi.mocked(invoke)
let pinia: ReturnType<typeof createPinia>
let mounted: ReturnType<typeof renderComponent> | undefined
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((a, b) => { resolve = a; reject = b })
  return { promise, resolve, reject }
}
function route(extra: Record<string, (args: any) => any> = {}) {
  const handlers: Record<string, (args: any) => any> = {
    initialize_app: () => undefined,
    get_provider: () => ({ id: 'sdkmax', name: 'Fixture', base_url: '', has_key: false, masked_key: null }),
    list_conversations: () => [], get_setting: () => null,
    get_last_active_project: () => null,
    list_tasks_for_project: () => [], detect_test_command: () => null,
    list_task_reviews: () => [], list_task_documents: () => [], list_task_self_checks: () => [],
    get_task_evidence_overview: () => overview(),
    get_completion_precheck: () => ({ ok: false, can_submit_acceptance: false }),
    list_messages: () => [], ...extra,
  }
  mockedInvoke.mockImplementation(async (command, args) => {
    if (!(command in handlers)) throw new Error(`Unexpected IPC: ${command}`)
    return handlers[command](args)
  })
}
const project: Project = {
  id: 'project', name: 'Fixture', local_path: 'C:/isolated-fixture', normalized_path: 'c:/isolated-fixture',
  source_type: 'local', source_url: null, current_branch: null, last_head_summary: null,
  purpose: null, long_term_goal: null, user_preferences: null, prohibitions: null,
  last_activity_at: null, last_opened_at: null, created_at: 'now', updated_at: 'now',
}
function task(change: Partial<Task> = {}): Task {
  return {
    id: 'task', project_id: project.id, title: 'Existing draft', state: 'draft',
    goal: 'same\nsame\n  original spacing', in_scope: 'one\ntwo', not_in_scope: 'no AI', done_criteria: 'reopen\ncheck',
    ai_kind: 'codex_cli', result_generation: 0, test_command_redacted: null, test_command_source: null,
    document_sync_pending: false, created_at: 'now', updated_at: 'now', ...change,
  }
}
function overview(): TaskEvidenceOverview {
  return { project_id: project.id, task_id: 'task', task_state: 'draft', result_generation: 0,
    pending_round: 1, needs_user_action: false, acceptance_ready: false, is_completed: false,
    timeline: [], file_changes: [], git_events: [], commands: [], tests: [], builds: [], acceptance_history: [] }
}
async function chat() {
  mounted = renderComponent(App, pinia)
  await settle()
  const store = useNovaStore()
  store.activeConversationId = 'conversation'
  const editor = all(mounted.root, n => n.kind === 'textarea')[0]
  return { root: mounted.root, editor, store }
}
async function core() {
  mounted = renderComponent(CoreLoopView, pinia)
  await settle()
  const store = useCoreLoopStore()
  store.currentProject = project
  await nextTick()
  return { root: mounted.root, store }
}
function requirements(root: TestNode) {
  const region = all(root, n => n.props.class === 'original-requirements')[0]
  expect(region).toBeDefined()
  expect(all(region, n => ['input', 'textarea', 'button'].includes(n.kind))).toHaveLength(0)
  return all(region, n => n.props.class === 'requirement-text').map(n => n.textContent)
}
function expectNoWrites(allowCreate = false) {
  const calls = mockedInvoke.mock.calls.map(c => c[0])
  const reads = ['get_last_active_project', 'list_tasks_for_project', 'detect_test_command',
    'list_task_reviews', 'list_task_documents', 'list_task_self_checks',
    'get_task_evidence_overview', 'get_completion_precheck']
  for (const command of calls) {
    expect([...reads, ...(allowCreate ? ['create_task'] : [])]).toContain(command)
  }
}

beforeEach(() => {
  pinia = createPinia(); setActivePinia(pinia); mockedInvoke.mockReset()
  // Only needed by Vue's v-model focus checks; this renderer has no browser DOM.
  vi.stubGlobal('document', { activeElement: null })
  vi.stubGlobal('Document', class {})
  vi.stubGlobal('ShadowRoot', class {})
  route()
})
afterEach(() => { mounted?.unmount(); mounted = undefined; vi.unstubAllGlobals() })

describe('IA-01 actual chat component events and IPC acceptance', () => {
  it.each(['', '  keep this draft\nsecond line  '])('shows the missing-model error, retains exact draft %j and never invokes send_chat_message', async draft => {
    const { root, editor } = await chat()
    await input(editor, draft)
    await click(button(root, '发送'))
    expect(root.textContent).toContain('请先选择模型后再发送消息')
    expect(editor.value).toBe(draft)
    expect(mockedInvoke.mock.calls.map(c => c[0])).not.toContain('send_chat_message')
  })
  it('keeps input until IPC acceptance, prevents repeat submission, then clears the accepted draft', async () => {
    const gate = deferred<{ assistant_message_id: string }>()
    route({ send_chat_message: () => gate.promise })
    const { root, editor, store } = await chat(); store.selectedModelId = 'mock-model'
    await input(editor, 'hello')
    const sending = click(button(root, '发送'))
    await settle()
    expect(editor.value).toBe('hello')
    // Ctrl+Enter also reaches the same handler even while the send button is hidden.
    await editor.props.onKeydown({ key: 'Enter', ctrlKey: true, preventDefault() {} })
    expect(mockedInvoke.mock.calls.filter(c => c[0] === 'send_chat_message')).toHaveLength(1)
    gate.resolve({ assistant_message_id: 'accepted-id' }); await sending
    expect(editor.value).toBe('')
    expect(store.sending).toBe(true)
  })
  it.each(['create_conversation', 'send_chat_message'])('retains the draft and displays a %s refusal', async command => {
    route({ [command]: () => { throw new Error('mock refusal') } })
    const { root, editor, store } = await chat(); store.selectedModelId = 'mock-model'
    if (command === 'create_conversation') store.activeConversationId = null
    await input(editor, 'not accepted'); await click(button(root, '发送'))
    expect(editor.value).toBe('not accepted'); expect(root.textContent).toContain('mock refusal')
    expect(store.sending).toBe(false)
  })
  it('clears an accepted send even if the subsequent message refresh fails, without unlocking its stream', async () => {
    const { root, editor, store } = await chat(); store.selectedModelId = 'mock-model'
    route({ send_chat_message: () => ({ assistant_message_id: 'accepted-id' }), list_messages: () => { throw new Error('read failed') } })
    await input(editor, 'already accepted'); await click(button(root, '发送'))
    expect(editor.value).toBe(''); expect(store.sending).toBe(true)
    expect(root.textContent).toContain('消息已发送，刷新消息列表失败')
    expect(mockedInvoke.mock.calls.filter(c => c[0] === 'send_chat_message')).toHaveLength(1)
  })
  it('does not clear a new draft typed during acceptance', async () => {
    const gate = deferred<{ assistant_message_id: string }>()
    route({ send_chat_message: () => gate.promise })
    const { root, editor, store } = await chat(); store.selectedModelId = 'mock-model'
    await input(editor, 'first'); const pending = click(button(root, '发送')); await settle()
    await input(editor, 'new draft'); gate.resolve({ assistant_message_id: 'accepted-id' }); await pending
    expect(editor.value).toBe('new draft')
  })
})

describe('IA-02 rendered verification status and attribution', () => {
  it.each([
    [undefined, '尚未运行', 'hint'], ['', '尚未运行', 'hint'], ['not_run', '尚未运行', 'hint'],
    ['planned', '已计划', 'hint'], ['started', '进行中', 'hint'], ['running', '进行中', 'hint'],
    ['failed', '失败', 'bad'], ['cancelled', '已取消', 'bad'], ['timed_out', '超时', 'bad'],
    ['insufficient_evidence', '证据不足', 'bad'], ['passed', '已通过', 'ok'],
    ['not_applicable', '不适用（依据已核验）', 'hint'],
    ['no_tests_applicable', '不适用依据尚未核验', 'bad'], ['no_build_applicable', '不适用依据尚未核验', 'bad'],
    ['mystery', '未知状态（mystery）', 'hint'],
  ])('maps %s to accurate text and style for both tests and builds', async (status, label, tone) => {
    const { root, store } = await core(); store.currentTask = task()
    const evidence = overview()
    if (status !== undefined) {
      evidence.tests = [{ status, is_current_generation: true, is_current_attempt: true } as TestEvidenceEntry]
      evidence.builds = [{ status, is_current_generation: true, is_current_attempt: true } as BuildEvidenceEntry]
    }
    store.evidenceOverview = evidence; await settle()
    for (const prefix of ['测试：', '构建：']) {
      const row = all(root, n => n.kind === 'p' && n.textContent.startsWith(prefix))[0]
      expect(row.textContent).toBe(prefix + label)
      expect(row.children.find(n => n.kind === 'span')?.props.class).toBe(tone)
    }
  })
  it('never promotes old generation/attempt evidence to a current success and preserves history markers', async () => {
    const { root, store } = await core(); store.currentTask = task(); const evidence = overview()
    evidence.tests = [{ status: 'passed', is_current_generation: false, is_current_attempt: true } as TestEvidenceEntry]
    evidence.builds = [{ status: 'passed', is_current_generation: true, is_current_attempt: false } as BuildEvidenceEntry]
    evidence.timeline = [{ id: 'old', title: 'Old test', status: 'passed', is_historical: true, needs_user_action: false } as any]
    store.evidenceOverview = evidence; await settle()
    expect(root.textContent).toContain('测试：尚未运行'); expect(root.textContent).toContain('构建：尚未运行')
    expect(root.textContent).toContain('（历史）')
  })
})

describe('IA-03 existing requirements are independent read-only data', () => {
  it('switches draft selection through the actual task button without refilling the new-task form or writing', async () => {
    const { root, store } = await core(); const existing = task(); store.tasks = [existing]
    await settle(); await click(button(root, 'Existing draft — 草稿'))
    expect(requirements(root)).toEqual([existing.goal, existing.in_scope, existing.not_in_scope, existing.done_criteria])
    const form = all(root, n => n.kind === 'textarea' && n.props.placeholder === '目标 / 需求描述')[0]
    expect(form.value).toBe(''); await input(form, 'unsubmitted new requirement')
    const other = task({ id: 'other', title: 'Other', goal: 'other goal' }); store.tasks.push(other); await settle()
    await click(button(root, 'Other — 草稿'))
    expect(requirements(root)[0]).toBe('other goal'); expect(form.value).toBe('unsubmitted new requirement')
    expectNoWrites()
  })
  it('shows the created task after the new form clears, without confirming or generating a snapshot', async () => {
    const created = task({ title: 'Created' })
    route({ create_task: () => created, list_tasks_for_project: () => [created] })
    const { root } = await core()
    const title = all(root, n => n.kind === 'input' && n.props.placeholder === '任务标题')[0]
    await input(title, 'Created')
    const editors = all(root, n => n.kind === 'textarea')
    for (const [index, value] of [created.goal, created.in_scope, created.not_in_scope, created.done_criteria].entries()) {
      await input(editors[index], value!)
    }
    await click(button(root, '创建任务'))
    expect(title.value).toBe(''); expect(requirements(root)[0]).toBe(created.goal)
    expect(editors.slice(0, 4).map(n => n.value)).toEqual(['', '', '', ''])
    expect(mockedInvoke).toHaveBeenCalledWith('create_task', { request: expect.objectContaining({
      goal: created.goal, in_scope: created.in_scope, not_in_scope: created.not_in_scope, done_criteria: created.done_criteria,
    }) })
    expectNoWrites(true)
    expect(mockedInvoke.mock.calls.filter(c => c[0] === 'create_task')).toHaveLength(1)
  })
  it('reloads a saved draft through restoreLastSession and renders exact text without writes', async () => {
    const saved = task()
    route({ get_last_active_project: () => project, list_tasks_for_project: () => [saved] })
    mounted = renderComponent(CoreLoopView, pinia); await settle()
    expect(requirements(mounted.root)).toEqual([saved.goal, saved.in_scope, saved.not_in_scope, saved.done_criteria])
    expectNoWrites()
  })
  it.each(['completed', 'failed', 'cancelled'] as const)('renders a %s task read-only while keeping snapshot/confirmation actions unavailable', async state => {
    const { root, store } = await core(); const existing = task({ state }); store.tasks = [existing]; await settle()
    const labels = { completed: '已完成', failed: '失败', cancelled: '已取消' }
    await click(button(root, `Existing draft — ${labels[state]}`))
    expect(requirements(root)).toEqual([existing.goal, existing.in_scope, existing.not_in_scope, existing.done_criteria])
    expect(root.textContent).not.toContain('确认沿用，生成需求清单')
    expect(all(root, n => n.kind === 'button' && n.textContent === '我已确认需求，继续')).toHaveLength(0)
    expectNoWrites()
  })
})


describe('RAV-01 progress and blocking feedback', () => {
  it('refresh evidence also updates the task state and exposes blocked details', async () => {
    const updated = task({ state: 'needs_user_action' })
    route({
      list_tasks_for_project: () => [updated],
      get_pending_round: () => 1,
      get_task_evidence_overview: () => ({ ...overview(), task_state: 'needs_user_action' }),
      list_task_self_checks: () => [{ id: 'check', task_id: 'task', round: 1, result_attempt: 1,
        parse_status: 'valid', self_reported_status: 'self_reported_blocked',
        blocking_issues_json: '["helper_unknown_error: setup refresh had errors"]',
        unfinished_items_json: '[]', check_results_json: '[]', created_at: 'now' }],
    })
    const { root, store } = await core()
    store.currentTask = task({ state: 'running' })
    store.evidenceOverview = overview()
    await nextTick()
    await click(button(root, '刷新时间线与证据'))
    await settle()
    expect(store.currentTask.state).toBe('needs_user_action')
    expect(root.textContent).toContain('需要用户处理')
    expect(root.textContent).toContain('helper_unknown_error: setup refresh had errors')
    expect(root.textContent).toContain('已刷新任务状态、时间线与证据')
    expect(mockedInvoke.mock.calls.some(c => c[0] === 'start_core_loop')).toBe(false)
  })

  it('shows refresh failure instead of silently swallowing it', async () => {
    route({ list_tasks_for_project: () => { throw new Error('read failed') } })
    const { root, store } = await core()
    store.currentTask = task({ state: 'running' })
    store.evidenceOverview = overview()
    await nextTick()
    await click(button(root, '刷新时间线与证据'))
    await settle()
    expect(root.textContent).toContain('刷新任务失败')
    expect(root.textContent).toContain('read failed')
    expect(root.textContent).not.toContain('已刷新任务状态、时间线与证据')
  })
})


describe('RAV-01 background failure notification', () => {
  it('shows the selected task error and refreshes without starting a task', async () => {
    let notify: ((event: any) => void) | undefined
    vi.mocked(listen).mockImplementationOnce(async (_name, handler) => {
      notify = handler
      return () => {}
    })
    route({ list_tasks_for_project: () => [task({ state: 'needs_user_action' })] })
    const { root, store } = await core()
    store.currentTask = task({ state: 'verifying' })
    notify?.({ payload: { task_id: 'task', error: '核心循环执行线程异常终止' } })
    await settle()
    expect(root.textContent).toContain('核心循环执行线程异常终止')
    expect(store.currentTask.state).toBe('needs_user_action')
    expect(mockedInvoke.mock.calls.some(c => c[0] === 'start_core_loop')).toBe(false)
  })

  it('ignores notifications from another task', async () => {
    let notify: ((event: any) => void) | undefined
    vi.mocked(listen).mockImplementationOnce(async (_name, handler) => {
      notify = handler
      return () => {}
    })
    route()
    const { root, store } = await core()
    store.currentTask = task({ state: 'running' })
    notify?.({ payload: { task_id: 'other-task', error: 'foreign failure' } })
    await settle()
    expect(root.textContent).not.toContain('foreign failure')
    expect(store.currentTask.state).toBe('running')
  })
})
