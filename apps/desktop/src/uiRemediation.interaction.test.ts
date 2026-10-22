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
    set_last_active_project: () => undefined,
    list_task_reviews: () => [], list_task_documents: () => [], list_task_self_checks: () => [],
    get_task_evidence_overview: () => overview(),
    get_completion_precheck: () => ({ ok: false, can_submit_acceptance: false }),
    get_verify_only_eligibility: () => null,
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
    'get_task_evidence_overview', 'get_completion_precheck', 'get_verify_only_eligibility']
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


describe('RAV-01 verify-only recovery', () => {
  const eligible = { result_generation: 0, round: 1, dev_attempt: 1, test_command: 'npm test' }
  it('shows an entry for an already confirmed command and dispatches only verify-only once', async () => {
    const pending = deferred<void>()
    route({ get_verify_only_eligibility: () => eligible, verify_existing_changes: () => pending.promise,
      list_tasks_for_project: () => [task({ state: 'verifying' })], get_pending_round: () => 1 })
    const { root, store } = await core()
    store.currentTask = task({ state: 'needs_user_action', test_command_source: 'user', test_command_redacted: 'npm test' })
    await settle()
    expect(root.textContent).toContain('沿用测试命令：npm test')
    const action = button(root, '重新验证现有修改')
    const request = click(action)
    await settle()
    expect(store.verifyingOnly).toBe(true)
    await store.verifyExistingChanges()
    expect(mockedInvoke.mock.calls.filter(c => c[0] === 'verify_existing_changes')).toHaveLength(1)
    expect(mockedInvoke.mock.calls.find(c => c[0] === 'verify_existing_changes')?.[1]).toEqual({ projectId: 'project', taskId: 'task', expected: eligible })
    pending.resolve(); await request
    expect(store.currentTask.state).toBe('verifying')
    expect(mockedInvoke.mock.calls.some(c => ['start_core_loop', 'get_core_loop_readiness', 'confirm_task_test_command'].includes(c[0]))).toBe(false)
  })
  it('hides the entry when backend rejects eligibility and discards late foreign-task results', async () => {
    const pending = deferred<typeof eligible>()
    route({ get_verify_only_eligibility: () => pending.promise })
    const { root, store } = await core()
    store.currentTask = task({ state: 'needs_user_action' }); await settle()
    store.currentTask = task({ id: 'other', state: 'completed' }); await settle()
    pending.resolve(eligible); await settle()
    expect(store.verifyOnlyEligibility).toBeNull()
    expect(all(root, n => n.kind === 'button' && n.textContent === '重新验证现有修改')).toHaveLength(0)
    await store.verifyExistingChanges()
    expect(mockedInvoke.mock.calls.some(c => c[0] === 'verify_existing_changes')).toBe(false)
  })
  it('shows request errors and never falls back to ordinary development', async () => {
    route({ get_verify_only_eligibility: () => eligible,
      verify_existing_changes: () => { throw new Error('任务验证条件已变化') } })
    const { root, store } = await core()
    store.currentTask = task({ state: 'needs_user_action', test_command_source: 'user', test_command_redacted: 'npm test' })
    await settle(); await click(button(root, '重新验证现有修改'))
    expect(root.textContent).toContain('任务验证条件已变化')
    expect(store.verifyingOnly).toBe(false)
    expect(mockedInvoke.mock.calls.some(c => c[0] === 'start_core_loop')).toBe(false)
  })
})


describe('RAV01 adopted evidence presentation', () => {
  it('shows adopted generation-zero verification and inspectable output at result generation one', async () => {
    const {root,store}=await core()
    store.currentTask=task({state:'awaiting_user_acceptance',result_generation:1})
    const view=overview(); view.result_generation=1;view.task_state='awaiting_user_acceptance'
    view.tests=[{id:'test',task_id:'task',project_id:'project',result_generation:0,round:1,result_attempt:1,
      status:'passed',test_command_redacted:'npm test',exit_code:0,stdout_redacted:'tests 2; pass 2',stderr_redacted:'',
      is_current_generation:false,is_current_attempt:true,is_adopted:true} as TestEvidenceEntry]
    view.builds=[{id:'build',task_id:'task',project_id:'project',result_generation:0,round:1,result_attempt:1,
      status:'passed',build_command_redacted:'npm run build',exit_code:0,stdout_redacted:'Built dist/index.html',stderr_redacted:'',
      is_current_generation:false,is_current_attempt:true,is_adopted:true} as BuildEvidenceEntry]
    view.file_changes=[{file_path:'index.html'},{file_path:'index.html'}] as any
    view.current_file_changes=[{file_path:'index.html'}] as any;view.file_snapshot_available=true
    store.evidenceOverview=view;await settle()
    expect(root.textContent).toContain('测试：已通过');expect(root.textContent).toContain('构建：已通过')
    expect(root.textContent).toContain('改动文件：1 个')
    expect(root.textContent).toContain('独立测试：npm test — exit 0')
    expect(root.textContent).toContain('独立构建：npm run build — exit 0')
    expect(root.textContent).toContain('当前结果依据')
    expect(root.textContent).toContain('tests 2; pass 2');expect(root.textContent).toContain('Built dist/index.html')
    expect(root.textContent).toContain('generation 0');expect(root.textContent).toContain('开发 attempt 1')
    expectNoWrites()
  })
  it('does not turn non-adopted historical passes into current success', async () => {
    const {root,store}=await core();store.currentTask=task({state:'needs_user_action',result_generation:2})
    const view=overview();view.tests=[{status:'passed',is_adopted:false,is_current_generation:false,is_current_attempt:true} as TestEvidenceEntry]
    view.builds=[{status:'passed',is_adopted:false,is_current_generation:false,is_current_attempt:false} as BuildEvidenceEntry]
    store.evidenceOverview=view;await settle()
    expect(root.textContent).toContain('测试：尚未运行');expect(root.textContent).toContain('构建：尚未运行')
    expect(root.textContent).toContain('执行记录（非当前采用）')
    expect(mockedInvoke.mock.calls.some(c=>['start_core_loop','verify_existing_changes','accept_task_result'].includes(c[0]))).toBe(false)
  })
})


describe('RAV01 document regeneration entry', () => {
  const doc = { document_type: 'TEST_REPORT', display_name: '测试报告', exists: true, is_optional_diagnostic: false }
  async function ready() {
    const result = await core()
    result.store.currentTask = task({ state: 'awaiting_user_acceptance' })
    result.store.taskDocuments = [doc]
    await settle()
    return result
  }
  function onlyDocumentWrites() {
    const writes = mockedInvoke.mock.calls.map(c => c[0]).filter(c => !c.startsWith('get_') && !c.startsWith('list_') && c !== 'read_task_document' && c !== 'set_last_active_project')
    expect(writes).toEqual(['resync_task_documents'])
  }
  it('is visible without pending, shares one in-flight request with retry, refreshes list and open content', async () => {
    const gate = deferred<Task>()
    route({ resync_task_documents: () => gate.promise, list_task_documents: () => [doc],
      read_task_document: () => ({ ...doc, content: 'fresh exit0 pass2', error: null }) })
    const {root, store} = await ready()
    store.selectedDocument = {...doc, content: 'old 尚未触发', error: null}
    const request = click(button(root, '重新生成任务文档'))
    await settle()
    expect(button(root, '正在更新…').props.disabled).toBe(true)
    await store.resyncDocuments()
    expect(mockedInvoke.mock.calls.filter(c => c[0] === 'resync_task_documents')).toHaveLength(1)
    gate.resolve(task({state:'awaiting_user_acceptance'})); await request
    expect(root.textContent).toContain('任务文档已更新')
    expect(store.selectedDocument?.content).toBe('fresh exit0 pass2')
    expect(mockedInvoke.mock.calls.filter(c => c[0] === 'resync_task_documents')[0][1]).toEqual({projectId:'project',taskId:'task'})
    onlyDocumentWrites()
  })
  it('shows a failure, restores retry and never starts any execution', async () => {
    route({resync_task_documents: () => {throw new Error('磁盘不可写')} })
    const {root} = await ready()
    await click(button(root,'重新生成任务文档'))
    expect(root.textContent).toContain('磁盘不可写')
    expect(button(root,'重新生成任务文档').props.disabled).toBe(false)
    onlyDocumentWrites()
  })
  it.each(['task','project'])('discards old success/error and loading state after switching %s', async kind => {
    const gate = deferred<Task>()
    route({resync_task_documents: () => gate.promise})
    const {store} = await ready()
    const request = store.resyncDocuments()
    if (kind === 'task') store.selectTask(task({id:'other',state:'completed'}))
    else await store.selectProject({...project,id:'other-project'})
    await settle()
    expect(store.syncingDocuments).toBe(false)
    gate.resolve(task({document_sync_pending:true})); await request
    expect(store.documentSyncMessage).toBe('')
    expect(store.documentSyncError).toBe('')
    expect(store.taskDocuments).toEqual([])
    expect(store.selectedDocument).toBeNull()
    expect(store.currentTask?.id).not.toBe('task')
    onlyDocumentWrites()
  })
  it('discards a stale failure after switching task', async () => {
    const gate = deferred<Task>(); route({resync_task_documents: () => gate.promise})
    const {store} = await ready(); const request = store.resyncDocuments()
    store.selectTask(task({id:'other'})); gate.reject(new Error('old failure')); await request
    expect(store.documentSyncError).toBe(''); expect(store.syncingDocuments).toBe(false)
  })
  it.each(['queued','running','verifying','safe_fixing','draft','confirmed'] as const)('disables unsupported state %s even with documents', async state => {
    const {root,store} = await ready(); store.currentTask = task({state}); await settle()
    expect(button(root,'重新生成任务文档').props.disabled).toBe(true)
    await store.resyncDocuments()
    expect(mockedInvoke.mock.calls.map(c=>c[0])).not.toContain('resync_task_documents')
  })
  it('retains the existing pending retry even when all documents are missing', async () => {
    const gate=deferred<Task>(); route({resync_task_documents:()=>gate.promise})
    const {root,store}=await ready(); store.currentTask!.document_sync_pending=true; store.taskDocuments=[]; await settle()
    const request=click(button(root,'立即重试文档同步')); await settle()
    expect(button(root,'正在更新…').props.disabled).toBe(true)
    await store.resyncDocuments(); gate.resolve(task({state:'completed'})); await request
    onlyDocumentWrites()
  })
  it.each(['list', 'read'])('does not publish a late %s refresh after task navigation', async stage => {
    const gate=deferred<any>()
    route({resync_task_documents:()=>task({state:'completed'}),
      list_task_documents:(args)=>args.taskId==='task'?(stage==='list'?gate.promise:[doc]):[],
      read_task_document:()=>gate.promise})
    const {store}=await ready()
    store.selectedDocument={...doc,content:'old',error:null}
    const request=store.resyncDocuments(); await settle()
    store.selectTask(task({id:'other',state:'completed'})); await settle()
    gate.resolve(stage==='list'?[doc]:{...doc,content:'late stale',error:null})
    await request
    expect(store.selectedDocument).toBeNull()
    expect(store.loadingDocument).toBe(false)
    expect(store.taskDocuments).toEqual([])
    expect(store.documentSyncMessage).toBe('')
    expect(store.documentSyncError).toBe('')
    expect(store.syncingDocuments).toBe(false)
  })
  it('explains missing documents and rejects a programmatic invocation', async () => {
    const {root,store}=await ready(); store.taskDocuments=[]; await settle()
    expect(button(root,'重新生成任务文档').props.disabled).toBe(true)
    expect(root.textContent).toContain('当前任务尚无结果文档可更新')
    await store.resyncDocuments()
    expect(mockedInvoke.mock.calls.map(c=>c[0])).not.toContain('resync_task_documents')
  })

})
