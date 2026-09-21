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
})
