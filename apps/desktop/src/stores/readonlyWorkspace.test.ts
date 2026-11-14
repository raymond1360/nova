import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
import { invoke } from '@tauri-apps/api/core'
import { useReadonlyWorkspace, DRAFT_PREFIX } from './readonlyWorkspace'
import type { Project, Task, TaskEvidenceOverview } from '../types'
const mocked = vi.mocked(invoke)
const project = (id: string) => ({ id, name: id, local_path: `C:/${id}` }) as Project
const task = (id: string, p = 'p') => ({ id, project_id: p, title: id, state: 'running', result_generation: 2 }) as Task
function overview(id = 'a', p = 'p'): TaskEvidenceOverview {
  return { project_id: p, task_id: id, task_state: 'running', result_generation: 2, pending_round: 1,
    needs_user_action: false, acceptance_ready: false, is_completed: false,
    timeline: [{ id: 'event', title: 'Recorded', status: 'running', event_type: 'development', started_at: null, finished_at: null, result_generation: 2, round: 1, needs_user_action: false, is_historical: false }],
    file_changes: [], git_events: [], commands: [], tests: [], builds: [], acceptance_history: [] }
}
function wait<T>() { let resolve!: (v: T) => void, reject!: (e: unknown) => void; const promise = new Promise<T>((a, b) => { resolve = a; reject = b }); return { promise, resolve, reject } }
async function settle() { for (let i = 0; i < 30; i++) await Promise.resolve() }
let memory: Map<string, string>
beforeEach(() => {
  setActivePinia(createPinia()); memory = new Map(); mocked.mockReset()
  vi.stubGlobal('sessionStorage', { getItem: (k: string) => memory.get(k) ?? null, setItem: (k: string, v: string) => memory.set(k, v) })
  mocked.mockImplementation(async (cmd, args: any) => {
    if (cmd === 'list_projects') return [project('p'), project('q')] as any
    if (cmd === 'list_tasks_for_project') return [task('a', args.projectId), task('b', args.projectId)] as any
    if (cmd === 'get_task_evidence_overview') return overview(args.taskId, args.projectId) as any
    throw new Error('Forbidden IPC ' + cmd)
  })
})
afterEach(() => vi.unstubAllGlobals())
async function selected() { const s = useReadonlyWorkspace(); await s.activate(); await s.selectProject('p'); s.selectTask('a'); await settle(); return s }
describe('read-only workspace isolation', () => {
  it('only reads the three allowed IPCs; replaces timeline and retains raw backend facts', async () => {
    const s = await selected(); await s.refreshOverview(); await s.refreshOverview()
    expect(s.overview?.timeline).toHaveLength(1); expect(s.overview?.task_state).toBe('running')
    expect(new Set(mocked.mock.calls.map(c => c[0]))).toEqual(new Set(['list_projects', 'list_tasks_for_project', 'get_task_evidence_overview']))
    expect(s.lastSuccessfulReadAt).not.toBeNull()
  })
  it('latest project-list success/error/finally wins; disposed lists cannot write', async () => {
    const s = useReadonlyWorkspace(), old = wait<Project[]>(), newer = wait<Project[]>()
    mocked.mockImplementationOnce(() => old.promise).mockImplementationOnce(() => newer.promise)
    const a = s.activate(), b = s.loadProjects(); newer.resolve([project('new')]); await b
    old.reject(new Error('late')); await a
    expect(s.projects.map(x => x.id)).toEqual(['new']); expect(s.projectsError).toBe(''); expect(s.projectsLoading).toBe(false)
    const pending = wait<Project[]>(); mocked.mockImplementationOnce(() => pending.promise)
    const c = s.loadProjects(); s.dispose(); pending.resolve([project('late')]); await c
    expect(s.projects.map(x => x.id)).toEqual(['new']); expect(s.projectsLoading).toBe(false)
  })
  it('out-of-order task list success and failure cannot contaminate another project', async () => {
    const s = useReadonlyWorkspace(); await s.activate(); const old = wait<Task[]>()
    mocked.mockImplementationOnce(() => old.promise)
    const first = s.selectProject('p'); await s.selectProject('q'); old.reject('late'); await first
    expect(s.projectId).toBe('q'); expect(s.tasks.every(t => t.project_id === 'q')).toBe(true)
    expect(s.tasksError).toBe(''); expect(s.tasksLoading).toBe(false)
    const late = wait<Task[]>(); mocked.mockImplementationOnce(() => late.promise)
    const second = s.selectProject('p'); await s.selectProject('q'); late.resolve([task('wrong')]); await second
    expect(s.tasks.every(t => t.project_id === 'q')).toBe(true)
  })
  it('serializes overview requests globally and drains only the latest selected scope', async () => {
    const s = await selected(), old = wait<TaskEvidenceOverview>()
    mocked.mockImplementationOnce(() => old.promise)
    const reading = s.refreshOverview(); s.selectTask('b'); void s.refreshOverview(); void s.refreshOverview()
    expect(s.overview).toBeNull(); expect(mocked.mock.calls.filter(c => c[0] === 'get_task_evidence_overview')).toHaveLength(2)
    old.resolve(overview('a')); await reading; await settle()
    expect(s.taskId).toBe('b'); expect(s.overview?.task_id).toBe('b'); expect(s.overviewLoading).toBe(false)
    expect(mocked.mock.calls.filter(c => c[0] === 'get_task_evidence_overview')).toHaveLength(3)
  })
  it('late overview failure/finally cannot clear the new scope or mutate after dispose', async () => {
    const s = await selected(), old = wait<TaskEvidenceOverview>(); mocked.mockImplementationOnce(() => old.promise)
    const a = s.refreshOverview(); s.selectTask('b'); old.reject('old'); await a; await settle()
    expect(s.overview?.task_id).toBe('b'); expect(s.overviewError).toBe('')
    const pending = wait<TaskEvidenceOverview>(); mocked.mockImplementationOnce(() => pending.promise)
    const b = s.refreshOverview(); s.dispose(); pending.resolve(overview('b')); await b
    expect(s.overview).toBeNull(); expect(s.lastSuccessfulReadAt).toBeNull(); expect(s.overviewLoading).toBe(false)
  })
  it('refresh errors retain a visibly stale snapshot; successful refresh clears error', async () => {
    const s = await selected(), previous = s.lastSuccessfulReadAt
    mocked.mockRejectedValueOnce(new Error('private exception text'))
    await s.refreshOverview(); expect(s.overview?.task_id).toBe('a'); expect(s.overviewError).toBe('READ_FAILED'); expect(s.lastSuccessfulReadAt).toBe(previous)
    await s.refreshOverview(); expect(s.overviewError).toBe('')
  })
  it('rejects mismatched overview/task ownership and does not invent records on empty data', async () => {
    const s = await selected(); mocked.mockResolvedValueOnce(overview('other'))
    await s.refreshOverview(); expect(s.overviewError).toBe('READ_FAILED'); expect(s.overview?.task_id).toBe('a')
    mocked.mockResolvedValueOnce([task('foreign', 'q')]); await s.selectProject('p'); expect(s.tasks).toEqual([]); expect(s.tasksError).toBe('READ_FAILED')
    mocked.mockResolvedValueOnce([]); await s.loadProjects(); expect(s.projects).toEqual([])
  })
  it('keeps exact per-scope window drafts across switching, remount and same-window reload', async () => {
    const s = await selected(), text = '  ÄZ\r\nnext\n\n'
    s.editDraft(text); s.selectTask('b'); s.editDraft('B'); s.selectTask('a'); expect(s.draft).toBe(text)
    await s.selectProject('q'); s.selectTask('a'); expect(s.draft).toBe(''); s.editDraft('Q')
    s.dispose(); await s.activate(); await s.selectProject('p'); s.selectTask('a'); expect(s.draft).toBe(text)
    setActivePinia(createPinia()); const reloaded = await selected(); expect(reloaded.draft).toBe(text)
    expect([...memory.keys()].every(k => k.startsWith(DRAFT_PREFIX))).toBe(true)
  })
  it('cache read/write failure keeps memory draft and signals incomplete persistence', async () => {
    const s = await selected(); vi.stubGlobal('sessionStorage', { getItem: () => { throw Error('blocked') }, setItem: () => { throw Error('quota') } })
    s.selectTask('b'); expect(s.cacheFailed).toBe(true); s.editDraft('  exact\ntext'); expect(s.cacheFailed).toBe(true)
    s.selectTask('a'); s.selectTask('b'); expect(s.draft).toBe('  exact\ntext')
  })
})
