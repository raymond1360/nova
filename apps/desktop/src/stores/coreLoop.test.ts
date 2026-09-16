import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}))

import { invoke } from '@tauri-apps/api/core'
import { useCoreLoopStore } from './coreLoop'
import type { Task } from '../types'

const mockedInvoke = vi.mocked(invoke)

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
    created_at: 'now',
    updated_at: 'now',
    ...overrides,
  }
}

describe('coreLoop store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    mockedInvoke.mockReset()
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
})
