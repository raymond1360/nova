import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}))
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}))

import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { useCoreLoopStore } from './coreLoop'
import type { Task } from '../types'

const mockedInvoke = vi.mocked(invoke)
const mockedOpen = vi.mocked(open)

function routeInvoke(handlers: Record<string, (args: unknown) => unknown>) {
  mockedInvoke.mockImplementation(async (command: string, args?: unknown) => {
    if (!(command in handlers)) {
      throw new Error(`unexpected invoke: ${command}`)
    }
    return handlers[command](args)
  })
}

// v0.1.0 scope correction (Work Package 2 remediation, Major 1): a
// manually-resolvable promise, used to control exactly when a mocked
// backend call settles and in what order relative to other in-flight
// calls — the deterministic alternative to real timers/`setTimeout` for
// reproducing stale-response races.
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

// Drains the microtask queue a fixed number of times — deterministic
// (no wall-clock dependency, never flaky under CPU load) unlike a real
// `setTimeout`/sleep, and sufficient for a promise chain a small constant
// number of `await`s deep (this store's async actions are at most 2-3
// `await` hops deep per call) to fully settle after being resolved.
async function flushMicrotasks(times = 10) {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve()
  }
}

function fakeTask(overrides: Partial<Task> = {}): Task {
  return {
    id: 'task-1',
    project_id: 'proj-1',
    title: 'T',
    goal: null,
    in_scope: null,
    not_in_scope: null,
    done_criteria: null,
    ai_kind: 'codex_cli',
    state: 'draft',
    result_generation: 0,
    test_command_redacted: null,
    test_command_source: null,
    document_sync_pending: false,
    created_at: 'now',
    updated_at: 'now',
    ...overrides,
  }
}

function fakeProject(overrides: Partial<import('../types').Project> = {}): import('../types').Project {
  return {
    id: 'proj-1',
    name: 'P',
    local_path: 'C:/p',
    normalized_path: 'c:/p',
    source_type: 'local',
    source_url: null,
    current_branch: null,
    last_head_summary: null,
    purpose: null,
    long_term_goal: null,
    user_preferences: null,
    prohibitions: null,
    last_activity_at: null,
    created_at: 'now',
    updated_at: 'now',
    last_opened_at: null,
    ...overrides,
  }
}

describe('coreLoop store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    mockedInvoke.mockReset()
    mockedOpen.mockReset()
  })

  it('creates a task via a real backend call and stores the result', async () => {
    const store = useCoreLoopStore()
    store.currentProject = {
      id: 'proj-1',
      name: 'P',
      local_path: 'C:/p',
      normalized_path: 'c:/p',
      source_type: 'local',
      source_url: null,
      current_branch: null,
      last_head_summary: null,
      purpose: null,
      long_term_goal: null,
      user_preferences: null,
      prohibitions: null,
      last_activity_at: null,
      created_at: 'now',
      updated_at: 'now',
      last_opened_at: null,
    }
    routeInvoke({
      create_task: async () => fakeTask({ title: 'Add search box' }),
      list_tasks_for_project: async () => [fakeTask({ title: 'Add search box' })],
      detect_test_command: async () => 'cargo test',
    })

    await store.createTask({
      title: 'Add search box',
      goal: 'goal',
      in_scope: '',
      not_in_scope: '',
      done_criteria: '',
    })

    expect(store.currentTask?.title).toBe('Add search box')
    expect(store.tasks).toHaveLength(1)
  })

  it('surfaces a backend error in lastError instead of throwing silently', async () => {
    const store = useCoreLoopStore()
    store.currentProject = {
      id: 'proj-1',
      name: 'P',
      local_path: 'C:/p',
      normalized_path: 'c:/p',
      source_type: 'local',
      source_url: null,
      current_branch: null,
      last_head_summary: null,
      purpose: null,
      long_term_goal: null,
      user_preferences: null,
      prohibitions: null,
      last_activity_at: null,
      created_at: 'now',
      updated_at: 'now',
      last_opened_at: null,
    }
    routeInvoke({
      create_task: async () => {
        throw new Error('任务未设置有效的开发 AI 类型')
      },
    })

    await expect(
      store.createTask({ title: 'X', goal: '', in_scope: '', not_in_scope: '', done_criteria: '' }),
    ).rejects.toThrow()
    expect(store.lastError).toContain('任务未设置有效的开发 AI 类型')
  })

  it('does not start a task when CLI readiness reports not ready', async () => {
    const store = useCoreLoopStore()
    store.currentTask = fakeTask({ state: 'confirmed' })
    let startCalled = false
    routeInvoke({
      get_core_loop_readiness: async () => ({
        dev_kind: 'codex_cli',
        reviewer_kind: 'claude_cli',
        dev_ready: false,
        review_ready: false,
        dev_message: '未检测到 Codex CLI',
        review_message: '未检测到 Claude CLI',
      }),
      start_core_loop: async () => {
        startCalled = true
      },
    })

    await store.checkReadiness()
    expect(store.readiness?.dev_ready).toBe(false)
    // The view layer gates the actual start button on readiness; this
    // proves the store's own data is what that gate reads, and confirms
    // start_core_loop is never called by checkReadiness itself.
    expect(startCalled).toBe(false)
  })

  // --- Codex re-review remediation (Blocker 1): dev-only start gate ---
  // (`CODEX_PHASE3_SINGLE_AI_RUNTIME_REVIEW09211520.md`, Blocker 1's
  // required test coverage.) `canStart`/`startIfReady` must depend purely
  // on the *selected* developer AI's own readiness — the other AI's
  // `review_ready` (always diagnostic-only per `get_core_loop_readiness`'s
  // own Minor 2 fix) must never participate.

  it('dev_ready=true, review_ready=false still allows starting (Codex-only / Claude missing)', async () => {
    const store = useCoreLoopStore()
    store.currentTask = fakeTask({ state: 'confirmed' })
    store.currentProject = fakeProject()
    let startCalled = false
    routeInvoke({
      get_core_loop_readiness: async () => ({
        dev_kind: 'codex_cli',
        reviewer_kind: 'claude_cli',
        dev_ready: true,
        review_ready: false,
        dev_message: 'Codex CLI 就绪',
        review_message: '未检查（可选诊断，不影响启动）',
      }),
      start_core_loop: async () => {
        startCalled = true
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    const started = await store.startIfReady()

    expect(store.canStart).toBe(true)
    expect(started).toBe(true)
    expect(startCalled).toBe(true)
  })

  it('dev_ready=false, review_ready=true still forbids starting', async () => {
    const store = useCoreLoopStore()
    store.currentTask = fakeTask({ state: 'confirmed' })
    let startCalled = false
    routeInvoke({
      get_core_loop_readiness: async () => ({
        dev_kind: 'codex_cli',
        reviewer_kind: 'claude_cli',
        dev_ready: false,
        review_ready: true,
        dev_message: '未检测到 Codex CLI',
        review_message: 'Claude CLI 就绪',
      }),
      start_core_loop: async () => {
        startCalled = true
      },
    })

    const started = await store.startIfReady()

    expect(store.canStart).toBe(false)
    expect(started).toBe(false)
    expect(startCalled).toBe(false)
  })

  it('Codex ready, Claude missing (review_ready=false) allows starting', async () => {
    const store = useCoreLoopStore()
    store.currentTask = fakeTask({ state: 'confirmed', ai_kind: 'codex_cli' })
    store.currentProject = fakeProject()
    let startCalled = false
    routeInvoke({
      get_core_loop_readiness: async () => ({
        dev_kind: 'codex_cli',
        reviewer_kind: 'claude_cli',
        dev_ready: true,
        review_ready: false,
        dev_message: 'Codex CLI 就绪',
        review_message: '未检查（可选诊断，不影响启动）',
      }),
      start_core_loop: async () => {
        startCalled = true
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    const started = await store.startIfReady()

    expect(started).toBe(true)
    expect(startCalled).toBe(true)
  })

  it('Claude ready, Codex missing allows starting when Claude is the selected dev AI', async () => {
    const store = useCoreLoopStore()
    store.devAiKind = 'claude_cli'
    store.currentTask = fakeTask({ state: 'confirmed', ai_kind: 'claude_cli' })
    store.currentProject = fakeProject()
    let startCalled = false
    routeInvoke({
      get_core_loop_readiness: async () => ({
        dev_kind: 'claude_cli',
        reviewer_kind: 'codex_cli',
        dev_ready: true,
        review_ready: false,
        dev_message: 'Claude CLI 就绪',
        review_message: '未检查（可选诊断，不影响启动）',
      }),
      start_core_loop: async () => {
        startCalled = true
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    const started = await store.startIfReady()

    expect(started).toBe(true)
    expect(startCalled).toBe(true)
  })

  // v0.1.0 scope correction (Work Package 2, required test coverage items
  // 2 and 3): the specific *reason* the unselected AI is unavailable
  // (missing vs. untrusted vs. not logged in) must never matter — only
  // `dev_ready` for the *selected* AI ever participates in `canStart`.

  it('Codex ready, Claude untrusted (review_ready=false) allows starting', async () => {
    const store = useCoreLoopStore()
    store.currentTask = fakeTask({ state: 'confirmed', ai_kind: 'codex_cli' })
    store.currentProject = fakeProject()
    let startCalled = false
    routeInvoke({
      get_core_loop_readiness: async () => ({
        dev_kind: 'codex_cli',
        reviewer_kind: 'claude_cli',
        dev_ready: true,
        review_ready: false,
        dev_message: 'Codex CLI 就绪',
        review_message: '未检查（可选诊断，不影响启动）',
      }),
      start_core_loop: async () => {
        startCalled = true
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    const started = await store.startIfReady()

    expect(started).toBe(true)
    expect(startCalled).toBe(true)
  })

  it('Codex ready, Claude not logged in (review_ready=false) allows starting', async () => {
    const store = useCoreLoopStore()
    store.currentTask = fakeTask({ state: 'confirmed', ai_kind: 'codex_cli' })
    store.currentProject = fakeProject()
    let startCalled = false
    routeInvoke({
      get_core_loop_readiness: async () => ({
        dev_kind: 'codex_cli',
        reviewer_kind: 'claude_cli',
        dev_ready: true,
        review_ready: false,
        dev_message: 'Codex CLI 就绪',
        review_message: '未检查（可选诊断，不影响启动）',
      }),
      start_core_loop: async () => {
        startCalled = true
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    const started = await store.startIfReady()

    expect(started).toBe(true)
    expect(startCalled).toBe(true)
  })

  it('the unselected AIs readiness state never affects canStart, in either direction', () => {
    const store = useCoreLoopStore()
    store.readiness = {
      dev_kind: 'codex_cli',
      reviewer_kind: 'claude_cli',
      dev_ready: true,
      review_ready: true,
      dev_message: 'ok',
      review_message: 'also ok, but irrelevant',
    }
    expect(store.canStart).toBe(true)
    store.readiness = { ...store.readiness, review_ready: false }
    expect(store.canStart).toBe(true)
  })

  it('prevents a duplicate concurrent start_core_loop call while one is already in flight', async () => {
    const store = useCoreLoopStore()
    store.currentTask = fakeTask({ state: 'confirmed' })
    let startCalls = 0
    let resolveStart!: () => void
    const gate = new Promise<void>((resolve) => {
      resolveStart = resolve
    })
    routeInvoke({
      start_core_loop: async () => {
        startCalls += 1
        await gate
      },
      list_tasks_for_project: async () => [store.currentTask as Task],
      list_task_reviews: async () => [],
    })
    store.currentProject = {
      id: 'proj-1',
      name: 'P',
      local_path: 'C:/p',
      normalized_path: 'c:/p',
      source_type: 'local',
      source_url: null,
      current_branch: null,
      last_head_summary: null,
      purpose: null,
      long_term_goal: null,
      user_preferences: null,
      prohibitions: null,
      last_activity_at: null,
      created_at: 'now',
      updated_at: 'now',
      last_opened_at: null,
    }

    const first = store.startTask()
    const second = store.startTask()
    resolveStart()
    await Promise.all([first, second])

    expect(startCalls).toBe(1)
  })

  it('locks role selection once a task has left the draft/awaiting_confirmation stage', () => {
    const store = useCoreLoopStore()
    store.currentTask = fakeTask({ state: 'draft' })
    expect(store.canChangeRoles).toBe(true)
    store.currentTask = fakeTask({ state: 'running' })
    expect(store.canChangeRoles).toBe(false)
  })

  it('reviewerAiKind is always the opposite of devAiKind (role swap pairing)', () => {
    const store = useCoreLoopStore()
    store.devAiKind = 'codex_cli'
    expect(store.reviewerAiKind).toBe('claude_cli')
    store.devAiKind = 'claude_cli'
    expect(store.reviewerAiKind).toBe('codex_cli')
  })

  // --- Second re-review remediation ---

  it('opens a project via the real system folder picker, never a hand-typed path', async () => {
    const store = useCoreLoopStore()
    mockedOpen.mockResolvedValue('D:\\real\\picked\\folder')
    routeInvoke({
      create_project: async (args) => {
        expect((args as { request: { local_path: string } }).request.local_path).toBe(
          'D:\\real\\picked\\folder',
        )
        return fakeProject({ local_path: 'D:\\real\\picked\\folder' })
      },
      set_last_active_project: async () => fakeProject(),
      list_projects: async () => [],
      list_tasks_for_project: async () => [],
    })

    await store.pickAndOpenProjectFolder('My Project')

    expect(mockedOpen).toHaveBeenCalledWith({ directory: true, multiple: false })
    expect(store.currentProject?.local_path).toBe('D:\\real\\picked\\folder')
  })

  it('does nothing when the user cancels the folder picker (no selection)', async () => {
    const store = useCoreLoopStore()
    mockedOpen.mockResolvedValue(null)
    let createCalled = false
    routeInvoke({
      create_project: async () => {
        createCalled = true
        return fakeProject()
      },
    })

    await store.pickAndOpenProjectFolder('X')

    expect(createCalled).toBe(false)
    expect(store.currentProject).toBeNull()
  })

  it('confirmNoTestsApplicable sends no default reason and refreshes the task afterward', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    let sentReason: unknown = 'unset'
    routeInvoke({
      confirm_no_tests_applicable: async (args) => {
        sentReason = (args as { reason: unknown }).reason
        return { id: 'c1', project_id: 'proj-1', task_id: 'task-1', result_generation: 0, round: 1, confirmed_by: 'user', reason: null, confirmed_at: 'now', created_at: 'now' }
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      detect_test_command: async () => null,
    })

    await store.confirmNoTestsApplicable('')

    expect(sentReason).toBeNull()
    expect(store.currentTask?.state).toBe('queued')
  })

  // v0.1.0 scope correction (Work Package 2): the no-build confirmation
  // prompt and its backend-authoritative "is it needed" query.

  it('loadNoBuildConfirmationNeeded reflects the real backend query', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    routeInvoke({
      get_no_build_confirmation_needed: async () => true,
    })

    await store.loadNoBuildConfirmationNeeded()

    expect(store.noBuildConfirmationNeeded).toBe(true)
  })

  it('refreshCurrentTask loads noBuildConfirmationNeeded only while needs_user_action', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1', state: 'needs_user_action' })
    let queried = false
    routeInvoke({
      list_tasks_for_project: async () => [fakeTask({ id: 'task-1', state: 'needs_user_action' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
      detect_test_command: async () => null,
      get_no_build_confirmation_needed: async () => {
        queried = true
        return true
      },
    })

    await store.refreshCurrentTask()

    expect(queried).toBe(true)
    expect(store.noBuildConfirmationNeeded).toBe(true)
  })

  it('refreshCurrentTask resets noBuildConfirmationNeeded once the task leaves needs_user_action', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1', state: 'needs_user_action' })
    store.noBuildConfirmationNeeded = true
    routeInvoke({
      list_tasks_for_project: async () => [fakeTask({ id: 'task-1', state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    await store.refreshCurrentTask()

    expect(store.noBuildConfirmationNeeded).toBe(false)
  })

  // Manual verification (Work Package 2) found this gap in a live preview:
  // `selectTask` (used when a user clicks a task straight from the task
  // list, e.g. right after reopening a project) never called
  // `refreshCurrentTask`, so a task already sitting at `needs_user_action`
  // for a build-insufficient stall never got its no-build prompt queried
  // until the next poll — unlike the no-tests card, which is a synchronous
  // getter over fields already on `task` and so appeared immediately.
  it('selectTask itself queries noBuildConfirmationNeeded for an already-needs_user_action task', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    let queried = false
    routeInvoke({
      get_no_build_confirmation_needed: async () => {
        queried = true
        return true
      },
    })

    store.selectTask(fakeTask({ state: 'needs_user_action' }))
    await vi.waitFor(() => expect(store.noBuildConfirmationNeeded).toBe(true))

    expect(queried).toBe(true)
  })

  it('selectTask does not query noBuildConfirmationNeeded for a task in any other state', () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    routeInvoke({
      get_no_build_confirmation_needed: async () => {
        throw new Error('must not be called for a non-needs_user_action task')
      },
    })

    expect(() => store.selectTask(fakeTask({ state: 'running' }))).not.toThrow()
    expect(store.noBuildConfirmationNeeded).toBe(false)
  })

  // Required test coverage item 12: the frontend must never be able to
  // forge a `round` for the no-build confirmation call — the backend
  // (`confirm_no_build_applicable`) derives it itself, exactly like
  // `confirmNoTestsApplicable` is intentionally *not* mirrored here.
  it('confirmNoBuildApplicable never sends a round, only project/task/reason', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    let sentArgs: unknown = null
    routeInvoke({
      confirm_no_build_applicable: async (args) => {
        sentArgs = args
        return {
          id: 'nb1',
          project_id: 'proj-1',
          task_id: 'task-1',
          result_generation: 0,
          round: 1,
          confirmed_by: 'user',
          reason: null,
          confirmed_at: 'now',
          created_at: 'now',
        }
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    await store.confirmNoBuildApplicable('no build needed')

    expect(sentArgs).toMatchObject({
      projectId: 'proj-1',
      taskId: 'task-1',
      reason: 'no build needed',
    })
    expect(sentArgs).not.toHaveProperty('round')
    expect(store.currentTask?.state).toBe('queued')
  })

  it('confirmNoBuildApplicable sends no default reason for an empty string', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    let sentReason: unknown = 'unset'
    routeInvoke({
      confirm_no_build_applicable: async (args) => {
        sentReason = (args as { reason: unknown }).reason
        return {
          id: 'nb1',
          project_id: 'proj-1',
          task_id: 'task-1',
          result_generation: 0,
          round: 1,
          confirmed_by: 'user',
          reason: null,
          confirmed_at: 'now',
          created_at: 'now',
        }
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    await store.confirmNoBuildApplicable('')

    expect(sentReason).toBeNull()
  })

  // --- v0.1.0 scope correction (Work Package 2 remediation, Major 1):
  // deterministic stale-response races for `loadNoBuildConfirmationNeeded`
  // — every scenario controls response ordering explicitly via
  // `deferred()`, never real timers or `setTimeout`. -------------------

  it('Major 1 scenario A: a slow response for task A never applies after switching to task B', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const taskA = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    const taskB = fakeTask({ id: 'task-b', state: 'running' })
    const responseA = deferred<boolean>()
    routeInvoke({
      get_no_build_confirmation_needed: async (args) => {
        const taskId = (args as { taskId: string }).taskId
        if (taskId === 'task-a') return responseA.promise
        throw new Error(`unexpected task id: ${taskId}`)
      },
    })

    store.selectTask(taskA) // fires the query for A, left pending
    store.selectTask(taskB) // B is not needs_user_action: no new query

    // A's query finally resolves `true` — after the user has already
    // moved on to B.
    responseA.resolve(true)
    await flushMicrotasks()

    expect(store.currentTask?.id).toBe('task-b')
    expect(store.noBuildConfirmationNeeded).toBe(false)
  })

  it('Major 1 scenario B: a fast true result for B is never overwritten by a slower false result for A', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const taskA = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    const taskB = fakeTask({ id: 'task-b', state: 'needs_user_action' })
    const responseA = deferred<boolean>()
    const responseB = deferred<boolean>()
    routeInvoke({
      get_no_build_confirmation_needed: async (args) => {
        const taskId = (args as { taskId: string }).taskId
        if (taskId === 'task-a') return responseA.promise
        if (taskId === 'task-b') return responseB.promise
        throw new Error(`unexpected task id: ${taskId}`)
      },
    })

    store.selectTask(taskA) // fires the query for A, left pending
    store.selectTask(taskB) // fires the query for B

    // B resolves first (true) — the user is looking at B's prompt.
    responseB.resolve(true)
    await flushMicrotasks()
    expect(store.noBuildConfirmationNeeded).toBe(true)

    // A's slower response finally arrives (false) — it must never
    // overwrite B's already-applied `true`.
    responseA.resolve(false)
    await flushMicrotasks()

    expect(store.currentTask?.id).toBe('task-b')
    expect(store.noBuildConfirmationNeeded).toBe(true)
  })

  it('Major 1 scenario C: for the same task, only the latest of two overlapping queries is ever applied', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const task = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    store.currentTask = task
    const firstCall = deferred<boolean>()
    const secondCall = deferred<boolean>()
    let callCount = 0
    routeInvoke({
      get_no_build_confirmation_needed: async () => {
        callCount += 1
        return callCount === 1 ? firstCall.promise : secondCall.promise
      },
    })

    const firstQuery = store.loadNoBuildConfirmationNeeded()
    const secondQuery = store.loadNoBuildConfirmationNeeded()

    // The second (later-issued) request resolves first, with `true`.
    secondCall.resolve(true)
    await flushMicrotasks()
    expect(store.noBuildConfirmationNeeded).toBe(true)

    // The first (now-stale) request resolves after it, with `false` — it
    // must not undo the second request's result.
    firstCall.resolve(false)
    await flushMicrotasks()
    await Promise.all([firstQuery, secondQuery])

    expect(store.noBuildConfirmationNeeded).toBe(true)
  })

  it('Major 1 scenario D: a stale true response is discarded once the task has left needs_user_action', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const task = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    store.currentTask = task
    const response = deferred<boolean>()
    routeInvoke({
      get_no_build_confirmation_needed: async () => response.promise,
    })

    store.loadNoBuildConfirmationNeeded().catch(() => {})
    // The task moves on while the query is still in flight (e.g. a
    // background refresh discovered it was actually resumed).
    store.currentTask = { ...task, state: 'queued' }

    response.resolve(true)
    await flushMicrotasks()

    expect(store.noBuildConfirmationNeeded).toBe(false)
  })

  it('Major 1 scenario E: a failed request for task A never sets an error or state for task B', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const taskA = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    const taskB = fakeTask({ id: 'task-b', state: 'needs_user_action' })
    const responseA = deferred<boolean>()
    routeInvoke({
      get_no_build_confirmation_needed: async (args) => {
        const taskId = (args as { taskId: string }).taskId
        if (taskId === 'task-a') return responseA.promise
        if (taskId === 'task-b') return true
        throw new Error(`unexpected task id: ${taskId}`)
      },
    })

    store.selectTask(taskA) // fires the query for A, left pending
    store.selectTask(taskB) // fires and resolves the query for B (true)
    await flushMicrotasks()
    expect(store.noBuildConfirmationNeeded).toBe(true)
    expect(store.lastError).toBe('')

    // A's request finally fails — after the user has moved on to B.
    responseA.reject(new Error('A failed'))
    await flushMicrotasks()

    expect(store.currentTask?.id).toBe('task-b')
    expect(store.noBuildConfirmationNeeded).toBe(true)
    expect(store.lastError).toBe('')
  })

  // --- v0.1.0 scope correction (Work Package 2 remediation, Minor 1):
  // `confirmNoBuildApplicable` in-flight/duplicate-submit protection. ----

  it('confirmNoBuildApplicable: a rapid double click only invokes the backend once', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    let callCount = 0
    const response = deferred<NonNullable<unknown>>()
    routeInvoke({
      confirm_no_build_applicable: async () => {
        callCount += 1
        return response.promise
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    const first = store.confirmNoBuildApplicable('reason')
    const second = store.confirmNoBuildApplicable('reason') // the "double click"

    expect(callCount).toBe(1)
    response.resolve({
      id: 'nb1',
      project_id: 'proj-1',
      task_id: 'task-1',
      result_generation: 0,
      round: 1,
      confirmed_by: 'user',
      reason: null,
      confirmed_at: 'now',
      created_at: 'now',
    })
    await Promise.all([first, second])

    expect(callCount).toBe(1)
  })

  it('confirmNoBuildApplicable: store.confirmingNoBuild is true only while the request is in flight', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    const response = deferred<NonNullable<unknown>>()
    routeInvoke({
      confirm_no_build_applicable: async () => response.promise,
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    expect(store.confirmingNoBuild).toBe(false)
    const call = store.confirmNoBuildApplicable('reason')
    expect(store.confirmingNoBuild).toBe(true)

    response.resolve({
      id: 'nb1',
      project_id: 'proj-1',
      task_id: 'task-1',
      result_generation: 0,
      round: 1,
      confirmed_by: 'user',
      reason: null,
      confirmed_at: 'now',
      created_at: 'now',
    })
    await call

    expect(store.confirmingNoBuild).toBe(false)
  })

  it('confirmNoBuildApplicable: releases the in-flight guard after a failure, allowing a retry', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    let callCount = 0
    routeInvoke({
      confirm_no_build_applicable: async () => {
        callCount += 1
        if (callCount === 1) throw new Error('backend rejected')
        return {
          id: 'nb1',
          project_id: 'proj-1',
          task_id: 'task-1',
          result_generation: 0,
          round: 1,
          confirmed_by: 'user',
          reason: null,
          confirmed_at: 'now',
          created_at: 'now',
        }
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    await expect(store.confirmNoBuildApplicable('reason')).rejects.toThrow()
    expect(store.confirmingNoBuild).toBe(false)
    expect(store.lastError).not.toBe('')

    await store.confirmNoBuildApplicable('reason') // retry
    expect(callCount).toBe(2)
    expect(store.currentTask?.state).toBe('queued')
  })

  it('confirmNoBuildApplicable: switching tasks mid-request does not refresh or overwrite the new task', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const taskA = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    const taskB = fakeTask({ id: 'task-b', state: 'needs_user_action' })
    store.currentTask = taskA
    const response = deferred<NonNullable<unknown>>()
    let listTasksCallCount = 0
    routeInvoke({
      confirm_no_build_applicable: async () => response.promise,
      list_tasks_for_project: async () => {
        listTasksCallCount += 1
        return [taskB]
      },
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    const call = store.confirmNoBuildApplicable('reason for A').catch(() => {})
    // The user switches to B while A's confirmation is still in flight.
    store.selectTask(taskB)

    response.resolve({
      id: 'nb1',
      project_id: 'proj-1',
      task_id: 'task-a',
      result_generation: 0,
      round: 1,
      confirmed_by: 'user',
      reason: null,
      confirmed_at: 'now',
      created_at: 'now',
    })
    await call
    await flushMicrotasks()

    expect(store.currentTask?.id).toBe('task-b')
    expect(store.currentTask?.state).toBe('needs_user_action')
    expect(listTasksCallCount).toBe(0)
  })

  it('confirmNoBuildApplicable never sends round or result_generation, even on retry', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    const sentArgsPerCall: unknown[] = []
    routeInvoke({
      confirm_no_build_applicable: async (args) => {
        sentArgsPerCall.push(args)
        return {
          id: 'nb1',
          project_id: 'proj-1',
          task_id: 'task-1',
          result_generation: 0,
          round: 1,
          confirmed_by: 'user',
          reason: null,
          confirmed_at: 'now',
          created_at: 'now',
        }
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    await store.confirmNoBuildApplicable('first attempt')
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    await store.confirmNoBuildApplicable('retry')

    expect(sentArgsPerCall).toHaveLength(2)
    for (const args of sentArgsPerCall) {
      expect(args).not.toHaveProperty('round')
      expect(args).not.toHaveProperty('result_generation')
      expect(args).not.toHaveProperty('resultGeneration')
    }
  })

  // --- Work Package 3 final review remediation (Major 1): the no-tests
  // confirmation prompt and its backend-authoritative "is it needed"
  // query, mirroring the no-build block above exactly. ------------------

  it('loadNoTestsConfirmationNeeded reflects the real backend query', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    routeInvoke({
      get_no_tests_confirmation_needed: async () => true,
    })

    await store.loadNoTestsConfirmationNeeded()

    expect(store.noTestsConfirmationNeeded).toBe(true)
  })

  // Phase 3 final rereview remediation: Case C (confirmation already
  // recorded, the backend's test action already reset to `planned`, but a
  // crash/restart happened before the task ever transitioned out of
  // `needs_user_action`) used to be invisible to the UI even though the
  // production `confirm_no_tests_applicable` command could already recover
  // it — the backend query returned `false` and the frontend never knew to
  // show the recovery entry. The store itself does not (and must not) know
  // *why* the backend said `true`; these tests exist specifically to
  // document and lock in that a Case C-shaped `true` response is surfaced
  // through every real user-facing path a restart could take, exactly like
  // any other `true` response.

  it('loadNoTestsConfirmationNeeded surfaces Case C recovery (confirmed + reset, still needs_user_action)', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    // The backend's `no_tests_confirmation_needed` now returns `true` for
    // this exact half-success state — the store only needs to trust it.
    routeInvoke({
      get_no_tests_confirmation_needed: async () => true,
    })

    await store.loadNoTestsConfirmationNeeded()

    expect(store.noTestsConfirmationNeeded).toBe(true)
  })

  it('restoreLastSession surfaces the Case C recovery card after an app restart', async () => {
    const store = useCoreLoopStore()
    const stuckTask = fakeTask({ id: 'task-case-c', state: 'needs_user_action' })
    routeInvoke({
      get_last_active_project: async () => fakeProject(),
      list_tasks_for_project: async () => [stuckTask],
      list_task_reviews: async () => [],
      get_no_build_confirmation_needed: async () => false,
      // Simulates the exact Case C production state: a confirmation was
      // already recorded and the test action already reset to `planned`
      // before the process was interrupted — the real backend query would
      // now return `true` for this task on the very first poll after
      // restart, with no user action required to trigger it.
      get_no_tests_confirmation_needed: async () => true,
    })

    await store.restoreLastSession()

    expect(store.currentTask?.id).toBe('task-case-c')
    expect(store.noTestsConfirmationNeeded).toBe(true)
  })

  it('selectTask surfaces the Case C recovery card for an already-needs_user_action task', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    routeInvoke({
      get_no_build_confirmation_needed: async () => false,
      get_no_tests_confirmation_needed: async () => true,
    })

    store.selectTask(fakeTask({ state: 'needs_user_action' }))
    await vi.waitFor(() => expect(store.noTestsConfirmationNeeded).toBe(true))
  })

  it('user can still invoke confirmNoTestsApplicable from a surfaced Case C recovery card, with a clean payload', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    routeInvoke({
      get_no_tests_confirmation_needed: async () => true,
    })
    await store.loadNoTestsConfirmationNeeded()
    expect(store.noTestsConfirmationNeeded).toBe(true)

    let sentArgs: unknown = null
    routeInvoke({
      confirm_no_tests_applicable: async (args) => {
        sentArgs = args
        return {
          id: 'nt-case-c',
          project_id: 'proj-1',
          task_id: 'task-1',
          result_generation: 0,
          round: 1,
          confirmed_by: 'user',
          reason: null,
          confirmed_at: 'now',
          created_at: 'now',
        }
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    await store.confirmNoTestsApplicable('resuming after restart')

    expect(sentArgs).not.toHaveProperty('round')
    expect(sentArgs).not.toHaveProperty('result_generation')
    expect(sentArgs).not.toHaveProperty('resultGeneration')
    expect(sentArgs).not.toHaveProperty('actionStatus')
    expect(store.currentTask?.state).toBe('queued')
  })

  it('refreshCurrentTask loads noTestsConfirmationNeeded only while needs_user_action', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1', state: 'needs_user_action' })
    let queried = false
    routeInvoke({
      list_tasks_for_project: async () => [fakeTask({ id: 'task-1', state: 'needs_user_action' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
      detect_test_command: async () => null,
      get_no_build_confirmation_needed: async () => false,
      get_no_tests_confirmation_needed: async () => {
        queried = true
        return true
      },
    })

    await store.refreshCurrentTask()

    expect(queried).toBe(true)
    expect(store.noTestsConfirmationNeeded).toBe(true)
  })

  it('refreshCurrentTask resets noTestsConfirmationNeeded once the task leaves needs_user_action', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1', state: 'needs_user_action' })
    store.noTestsConfirmationNeeded = true
    routeInvoke({
      list_tasks_for_project: async () => [fakeTask({ id: 'task-1', state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    await store.refreshCurrentTask()

    expect(store.noTestsConfirmationNeeded).toBe(false)
  })

  it('selectTask itself queries noTestsConfirmationNeeded for an already-needs_user_action task', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    let queried = false
    routeInvoke({
      get_no_build_confirmation_needed: async () => false,
      get_no_tests_confirmation_needed: async () => {
        queried = true
        return true
      },
    })

    store.selectTask(fakeTask({ state: 'needs_user_action' }))
    await vi.waitFor(() => expect(store.noTestsConfirmationNeeded).toBe(true))

    expect(queried).toBe(true)
  })

  it('selectTask does not query noTestsConfirmationNeeded for a task in any other state', () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    routeInvoke({
      get_no_tests_confirmation_needed: async () => {
        throw new Error('must not be called for a non-needs_user_action task')
      },
    })

    expect(() => store.selectTask(fakeTask({ state: 'running' }))).not.toThrow()
    expect(store.noTestsConfirmationNeeded).toBe(false)
  })

  // Required test coverage item 27: the frontend must never be able to
  // forge a `round`/`result_generation` for the no-tests confirmation call
  // — the backend (`confirm_no_tests_applicable`) derives both itself.
  it('confirmNoTestsApplicable never sends round or result_generation, even on retry', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    const sentArgsPerCall: unknown[] = []
    routeInvoke({
      confirm_no_tests_applicable: async (args) => {
        sentArgsPerCall.push(args)
        return {
          id: 'nt1',
          project_id: 'proj-1',
          task_id: 'task-1',
          result_generation: 0,
          round: 1,
          confirmed_by: 'user',
          reason: null,
          confirmed_at: 'now',
          created_at: 'now',
        }
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    await store.confirmNoTestsApplicable('first attempt')
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    await store.confirmNoTestsApplicable('retry')

    expect(sentArgsPerCall).toHaveLength(2)
    for (const args of sentArgsPerCall) {
      expect(args).not.toHaveProperty('round')
      expect(args).not.toHaveProperty('result_generation')
      expect(args).not.toHaveProperty('resultGeneration')
    }
  })

  // --- deterministic stale-response races for
  // `loadNoTestsConfirmationNeeded` — every scenario controls response
  // ordering explicitly via `deferred()`, never real timers or
  // `setTimeout`, mirroring the no-build scenarios A-E exactly. ----------

  it('Major 1 (no-tests) scenario A: a slow response for task A never applies after switching to task B', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const taskA = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    const taskB = fakeTask({ id: 'task-b', state: 'running' })
    const responseA = deferred<boolean>()
    routeInvoke({
      get_no_tests_confirmation_needed: async (args) => {
        const taskId = (args as { taskId: string }).taskId
        if (taskId === 'task-a') return responseA.promise
        throw new Error(`unexpected task id: ${taskId}`)
      },
    })

    store.selectTask(taskA)
    store.selectTask(taskB)

    responseA.resolve(true)
    await flushMicrotasks()

    expect(store.currentTask?.id).toBe('task-b')
    expect(store.noTestsConfirmationNeeded).toBe(false)
  })

  it('Major 1 (no-tests) scenario B: a fast true result for B is never overwritten by a slower false result for A', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const taskA = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    const taskB = fakeTask({ id: 'task-b', state: 'needs_user_action' })
    const responseA = deferred<boolean>()
    const responseB = deferred<boolean>()
    routeInvoke({
      get_no_tests_confirmation_needed: async (args) => {
        const taskId = (args as { taskId: string }).taskId
        if (taskId === 'task-a') return responseA.promise
        if (taskId === 'task-b') return responseB.promise
        throw new Error(`unexpected task id: ${taskId}`)
      },
    })

    store.selectTask(taskA)
    store.selectTask(taskB)

    responseB.resolve(true)
    await flushMicrotasks()
    expect(store.noTestsConfirmationNeeded).toBe(true)

    responseA.resolve(false)
    await flushMicrotasks()

    expect(store.currentTask?.id).toBe('task-b')
    expect(store.noTestsConfirmationNeeded).toBe(true)
  })

  it('Major 1 (no-tests) scenario C: for the same task, only the latest of two overlapping queries is ever applied', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const task = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    store.currentTask = task
    const firstCall = deferred<boolean>()
    const secondCall = deferred<boolean>()
    let callCount = 0
    routeInvoke({
      get_no_tests_confirmation_needed: async () => {
        callCount += 1
        return callCount === 1 ? firstCall.promise : secondCall.promise
      },
    })

    const firstQuery = store.loadNoTestsConfirmationNeeded()
    const secondQuery = store.loadNoTestsConfirmationNeeded()

    secondCall.resolve(true)
    await flushMicrotasks()
    expect(store.noTestsConfirmationNeeded).toBe(true)

    firstCall.resolve(false)
    await flushMicrotasks()
    await Promise.all([firstQuery, secondQuery])

    expect(store.noTestsConfirmationNeeded).toBe(true)
  })

  it('Major 1 (no-tests) scenario D: a stale true response is discarded once the task has left needs_user_action', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const task = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    store.currentTask = task
    const response = deferred<boolean>()
    routeInvoke({
      get_no_tests_confirmation_needed: async () => response.promise,
    })

    store.loadNoTestsConfirmationNeeded().catch(() => {})
    store.currentTask = { ...task, state: 'queued' }

    response.resolve(true)
    await flushMicrotasks()

    expect(store.noTestsConfirmationNeeded).toBe(false)
  })

  it('Major 1 (no-tests) scenario E: a failed request for task A never sets an error or state for task B', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const taskA = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    const taskB = fakeTask({ id: 'task-b', state: 'needs_user_action' })
    const responseA = deferred<boolean>()
    routeInvoke({
      get_no_tests_confirmation_needed: async (args) => {
        const taskId = (args as { taskId: string }).taskId
        if (taskId === 'task-a') return responseA.promise
        if (taskId === 'task-b') return true
        throw new Error(`unexpected task id: ${taskId}`)
      },
    })

    store.selectTask(taskA)
    store.selectTask(taskB)
    await flushMicrotasks()
    expect(store.noTestsConfirmationNeeded).toBe(true)
    expect(store.lastError).toBe('')

    responseA.reject(new Error('A failed'))
    await flushMicrotasks()

    expect(store.currentTask?.id).toBe('task-b')
    expect(store.noTestsConfirmationNeeded).toBe(true)
    expect(store.lastError).toBe('')
  })

  // --- `confirmNoTestsApplicable` in-flight/duplicate-submit protection,
  // mirroring `confirmNoBuildApplicable`'s block exactly. ----------------

  it('confirmNoTestsApplicable: a rapid double click only invokes the backend once', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    let callCount = 0
    const response = deferred<NonNullable<unknown>>()
    routeInvoke({
      confirm_no_tests_applicable: async () => {
        callCount += 1
        return response.promise
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    const first = store.confirmNoTestsApplicable('reason')
    const second = store.confirmNoTestsApplicable('reason') // the "double click"

    expect(callCount).toBe(1)
    response.resolve({
      id: 'nt1',
      project_id: 'proj-1',
      task_id: 'task-1',
      result_generation: 0,
      round: 1,
      confirmed_by: 'user',
      reason: null,
      confirmed_at: 'now',
      created_at: 'now',
    })
    await Promise.all([first, second])

    expect(callCount).toBe(1)
  })

  it('confirmNoTestsApplicable: store.confirmingNoTests is true only while the request is in flight', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    const response = deferred<NonNullable<unknown>>()
    routeInvoke({
      confirm_no_tests_applicable: async () => response.promise,
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    expect(store.confirmingNoTests).toBe(false)
    const call = store.confirmNoTestsApplicable('reason')
    expect(store.confirmingNoTests).toBe(true)

    response.resolve({
      id: 'nt1',
      project_id: 'proj-1',
      task_id: 'task-1',
      result_generation: 0,
      round: 1,
      confirmed_by: 'user',
      reason: null,
      confirmed_at: 'now',
      created_at: 'now',
    })
    await call

    expect(store.confirmingNoTests).toBe(false)
  })

  it('confirmNoTestsApplicable: releases the in-flight guard after a failure, allowing a retry', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'needs_user_action' })
    let callCount = 0
    routeInvoke({
      confirm_no_tests_applicable: async () => {
        callCount += 1
        if (callCount === 1) throw new Error('backend rejected')
        return {
          id: 'nt1',
          project_id: 'proj-1',
          task_id: 'task-1',
          result_generation: 0,
          round: 1,
          confirmed_by: 'user',
          reason: null,
          confirmed_at: 'now',
          created_at: 'now',
        }
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'queued' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    await expect(store.confirmNoTestsApplicable('reason')).rejects.toThrow()
    expect(store.confirmingNoTests).toBe(false)
    expect(store.lastError).not.toBe('')

    await store.confirmNoTestsApplicable('reason') // retry
    expect(callCount).toBe(2)
    expect(store.currentTask?.state).toBe('queued')
  })

  it('confirmNoTestsApplicable: switching tasks mid-request does not refresh or overwrite the new task', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const taskA = fakeTask({ id: 'task-a', state: 'needs_user_action' })
    const taskB = fakeTask({ id: 'task-b', state: 'needs_user_action' })
    store.currentTask = taskA
    const response = deferred<NonNullable<unknown>>()
    let listTasksCallCount = 0
    routeInvoke({
      confirm_no_tests_applicable: async () => response.promise,
      list_tasks_for_project: async () => {
        listTasksCallCount += 1
        return [taskB]
      },
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
    })

    const call = store.confirmNoTestsApplicable('reason for A').catch(() => {})
    store.selectTask(taskB)

    response.resolve({
      id: 'nt1',
      project_id: 'proj-1',
      task_id: 'task-a',
      result_generation: 0,
      round: 1,
      confirmed_by: 'user',
      reason: null,
      confirmed_at: 'now',
      created_at: 'now',
    })
    await call
    await flushMicrotasks()

    expect(store.currentTask?.id).toBe('task-b')
    expect(store.currentTask?.state).toBe('needs_user_action')
    expect(listTasksCallCount).toBe(0)
  })

  // Codex re-review remediation (Major 1): `pendingRound` is sourced
  // exclusively from the backend's own Action Ledger-derived
  // `get_pending_round` command — the frontend must never infer it from
  // `reviews` (the exact stale logic the re-review flagged).
  it('loadPendingRound populates pendingRound from the backend, defaulting to 1', async () => {
    const store = useCoreLoopStore()
    expect(store.pendingRound).toBe(1)
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ result_generation: 0 })
    let requestedTaskId: unknown = null
    routeInvoke({
      get_pending_round: async (args) => {
        requestedTaskId = (args as { taskId: string }).taskId
        return 2
      },
    })

    await store.loadPendingRound()

    expect(store.pendingRound).toBe(2)
    expect(requestedTaskId).toBe('task-1')
  })

  it('refreshCurrentTask loads pendingRound from the backend, not from reviews', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1', state: 'needs_user_action' })
    routeInvoke({
      list_tasks_for_project: async () => [fakeTask({ id: 'task-1', state: 'needs_user_action' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 2,
      detect_test_command: async () => null,
    })

    await store.refreshCurrentTask()

    expect(store.pendingRound).toBe(2)
  })

  it('acceptResult calls the single atomic accept_task_result command, not the old two-step pattern', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'awaiting_user_acceptance' })
    let calledWith: unknown = null
    routeInvoke({
      accept_task_result: async (args) => {
        calledWith = args
        return fakeTask({ state: 'completed' })
      },
      list_tasks_for_project: async () => [fakeTask({ state: 'completed' })],
      list_task_reviews: async () => [],
    })

    await store.acceptResult(true, 'looks good')

    expect(calledWith).toMatchObject({
      projectId: 'proj-1',
      taskId: 'task-1',
      accepted: true,
      note: 'looks good',
    })
    expect(store.currentTask?.state).toBe('completed')
  })

  it('acceptResult(false) still goes through accept_task_result, not a bare record_user_acceptance', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ state: 'awaiting_user_acceptance' })
    routeInvoke({
      accept_task_result: async () => fakeTask({ state: 'needs_user_action' }),
      list_tasks_for_project: async () => [fakeTask({ state: 'needs_user_action' })],
      list_task_reviews: async () => [],
    })

    await store.acceptResult(false, '')

    expect(store.currentTask?.state).toBe('needs_user_action')
  })

  it('confirmCliBinaryPath resolves, confirms, and persists the setting for the correct kind', async () => {
    const store = useCoreLoopStore()
    const calls: string[] = []
    routeInvoke({
      confirm_cli_binary_path: async (args) => {
        calls.push('confirm_cli_binary_path')
        expect(args).toMatchObject({ kind: 'codex_cli', path: 'C:\\tools\\codex.exe' })
        return { trusted: true, canonical_path: 'C:\\tools\\codex.exe', file_size: 100, sha256_hex: 'abc', confirmed_at: 'now', mismatch_reason: null }
      },
      set_setting: async (args) => {
        calls.push('set_setting')
        expect(args).toMatchObject({ key: 'codex_binary_path', value: 'C:\\tools\\codex.exe' })
      },
    })

    await store.confirmCliBinaryPath('codex_cli', 'C:\\tools\\codex.exe')

    expect(calls).toEqual(['confirm_cli_binary_path', 'set_setting'])
    expect(store.codexTrust?.trusted).toBe(true)
    expect(store.claudeTrust).toBeNull()
  })

  it('restoreLastSession loads the last active project and its unfinished task', async () => {
    const store = useCoreLoopStore()
    routeInvoke({
      get_last_active_project: async () => fakeProject(),
      list_tasks_for_project: async () => [
        fakeTask({ id: 'old-done', state: 'completed' }),
        fakeTask({ id: 'still-going', state: 'running' }),
      ],
      list_task_reviews: async () => [],
    })

    await store.restoreLastSession()

    expect(store.currentProject?.id).toBe('proj-1')
    expect(store.currentTask?.id).toBe('still-going')
  })

  it('restoreLastSession leaves currentTask unset when nothing is unfinished', async () => {
    const store = useCoreLoopStore()
    routeInvoke({
      get_last_active_project: async () => fakeProject(),
      list_tasks_for_project: async () => [fakeTask({ state: 'completed' })],
    })

    await store.restoreLastSession()

    expect(store.currentTask).toBeNull()
  })

  // --- Phase 4 (evidence and document gate) ---------------------------

  function fakeEvidenceOverview(
    overrides: Partial<import('../types').TaskEvidenceOverview> = {},
  ): import('../types').TaskEvidenceOverview {
    return {
      project_id: 'proj-1',
      task_id: 'task-1',
      task_state: 'running',
      result_generation: 0,
      pending_round: 1,
      needs_user_action: false,
      acceptance_ready: false,
      is_completed: false,
      timeline: [],
      file_changes: [],
      git_events: [],
      commands: [],
      tests: [],
      builds: [],
      acceptance_history: [],
      ...overrides,
    }
  }

  function fakeDocumentInfo(
    overrides: Partial<import('../types').TaskDocumentInfo> = {},
  ): import('../types').TaskDocumentInfo {
    return {
      document_type: 'REQUIREMENT',
      display_name: '需求确认单',
      exists: true,
      is_optional_diagnostic: false,
      ...overrides,
    }
  }

  it('selectTask loads the real evidence overview and document list for the selected task', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const overview = fakeEvidenceOverview({ task_id: 'task-1' })
    routeInvoke({
      get_task_evidence_overview: async () => overview,
      list_task_documents: async () => [fakeDocumentInfo()],
    })

    store.selectTask(fakeTask({ id: 'task-1', state: 'running' }))
    await vi.waitFor(() => expect(store.evidenceOverview).not.toBeNull())

    expect(store.evidenceOverview).toEqual(overview)
    expect(store.taskDocuments).toHaveLength(1)
  })

  it('refreshCurrentTask reloads the evidence overview and document list from the backend', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1', state: 'running' })
    const refreshedOverview = fakeEvidenceOverview({
      task_id: 'task-1',
      timeline: [
        {
          id: 'evt-1',
          event_type: 'task_created',
          title: '任务已创建',
          status: 'created',
          started_at: 'now',
          finished_at: 'now',
          result_generation: null,
          round: null,
          needs_user_action: false,
          is_historical: false,
        },
      ],
    })
    routeInvoke({
      list_tasks_for_project: async () => [fakeTask({ id: 'task-1', state: 'running' })],
      list_task_reviews: async () => [],
      get_pending_round: async () => 1,
      get_task_evidence_overview: async () => refreshedOverview,
      list_task_documents: async () => [fakeDocumentInfo({ exists: false })],
    })

    await store.refreshCurrentTask()

    expect(store.evidenceOverview?.timeline).toHaveLength(1)
    expect(store.taskDocuments[0].exists).toBe(false)
  })

  it('restoreLastSession restores the evidence and document view for the recovered task', async () => {
    const store = useCoreLoopStore()
    const overview = fakeEvidenceOverview({ task_id: 'still-going' })
    routeInvoke({
      get_last_active_project: async () => fakeProject(),
      list_tasks_for_project: async () => [fakeTask({ id: 'still-going', state: 'running' })],
      list_task_reviews: async () => [],
      get_task_evidence_overview: async () => overview,
      list_task_documents: async () => [fakeDocumentInfo()],
    })

    await store.restoreLastSession()

    expect(store.currentTask?.id).toBe('still-going')
    expect(store.evidenceOverview).toEqual(overview)
    expect(store.taskDocuments).toHaveLength(1)
  })

  it('a slow evidence-overview response for task A never overwrites task B after switching', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    const taskA = fakeTask({ id: 'task-a', state: 'running' })
    const taskB = fakeTask({ id: 'task-b', state: 'running' })
    const responseA = deferred<import('../types').TaskEvidenceOverview>()
    routeInvoke({
      get_task_evidence_overview: async (args) => {
        const taskId = (args as { taskId: string }).taskId
        if (taskId === 'task-a') return responseA.promise
        return fakeEvidenceOverview({ task_id: 'task-b' })
      },
      list_task_documents: async () => [],
    })

    store.selectTask(taskA)
    store.selectTask(taskB)
    await flushMicrotasks()
    const afterB = store.evidenceOverview

    responseA.resolve(fakeEvidenceOverview({ task_id: 'task-a' }))
    await flushMicrotasks()

    expect(store.currentTask?.id).toBe('task-b')
    expect(store.evidenceOverview).toEqual(afterB)
    expect(store.evidenceOverview?.task_id).toBe('task-b')
  })

  it('repeated loadEvidenceOverview calls replace the timeline wholesale, never duplicating entries', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1' })
    const oneEntryOverview = fakeEvidenceOverview({
      timeline: [
        {
          id: 'evt-1',
          event_type: 'task_created',
          title: '任务已创建',
          status: 'created',
          started_at: 'now',
          finished_at: 'now',
          result_generation: null,
          round: null,
          needs_user_action: false,
          is_historical: false,
        },
      ],
    })
    routeInvoke({ get_task_evidence_overview: async () => oneEntryOverview })

    await store.loadEvidenceOverview()
    await store.loadEvidenceOverview()
    await store.loadEvidenceOverview()

    expect(store.evidenceOverview?.timeline).toHaveLength(1)
  })

  it('a failed evidence-overview query leaves evidenceOverview alone instead of showing a fake result', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1' })
    store.evidenceOverview = fakeEvidenceOverview({ task_id: 'task-1', acceptance_ready: true })
    routeInvoke({
      get_task_evidence_overview: async () => {
        throw new Error('backend unavailable')
      },
    })

    await expect(store.loadEvidenceOverview()).rejects.toThrow('backend unavailable')

    expect(store.evidenceOverview?.acceptance_ready).toBe(true)
  })

  it('document list rejects an unexpected shape but never invents a fake successful list', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1' })
    routeInvoke({
      list_task_documents: async () => {
        throw new Error('unknown document type')
      },
    })

    await expect(store.loadTaskDocuments()).rejects.toThrow()
    expect(store.taskDocuments).toEqual([])
  })

  it('viewTaskDocument sends only the fixed document_type from the list, never a free-typed path', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1' })
    let sentArgs: unknown = null
    routeInvoke({
      read_task_document: async (args) => {
        sentArgs = args
        return {
          document_type: 'REQUIREMENT',
          display_name: '需求确认单',
          exists: true,
          content: '# REQUIREMENT\n...',
          error: null,
        }
      },
    })

    await store.viewTaskDocument('REQUIREMENT')

    expect(sentArgs).toEqual({
      projectId: 'proj-1',
      taskId: 'task-1',
      documentType: 'REQUIREMENT',
    })
    expect(Object.keys(sentArgs as object)).not.toContain('path')
    expect(store.selectedDocument?.content).toContain('REQUIREMENT')
  })

  it('a stale document-view response never overwrites a newer one for a different document', async () => {
    const store = useCoreLoopStore()
    store.currentProject = fakeProject()
    store.currentTask = fakeTask({ id: 'task-1' })
    const slowFirst = deferred<import('../types').TaskDocumentContent>()
    routeInvoke({
      read_task_document: async (args) => {
        const documentType = (args as { documentType: string }).documentType
        if (documentType === 'REQUIREMENT') return slowFirst.promise
        return {
          document_type: 'FINAL_RESULT',
          display_name: '最终结果',
          exists: true,
          content: 'final content',
          error: null,
        }
      },
    })

    const firstCall = store.viewTaskDocument('REQUIREMENT')
    await store.viewTaskDocument('FINAL_RESULT')
    slowFirst.resolve({
      document_type: 'REQUIREMENT',
      display_name: '需求确认单',
      exists: true,
      content: 'stale requirement content',
      error: null,
    })
    await firstCall
    await flushMicrotasks()

    expect(store.selectedDocument?.document_type).toBe('FINAL_RESULT')
  })

  it('clearing selectedDocument (closing the viewer) does not require a backend call', async () => {
    const store = useCoreLoopStore()
    store.selectedDocument = {
      document_type: 'REQUIREMENT',
      display_name: '需求确认单',
      exists: true,
      content: 'x',
      error: null,
    }

    store.selectedDocument = null

    expect(store.selectedDocument).toBeNull()
  })
})
