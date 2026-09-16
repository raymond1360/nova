import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import type { CoreLoopReadiness, Project, Task, TaskReview, TaskState } from '../types'

// Phase 3 remediation (Major 1): the minimal, real-backend-connected core
// loop UI's store. Deliberately does not attempt the full Phase 4 timeline/
// evidence center — only what a non-technical user needs to select a
// project, confirm a requirement, pick a role combination, see CLI
// readiness, confirm a test command, start/cancel a run, and accept/reject
// the result.
export const useCoreLoopStore = defineStore('coreLoop', {
  state: () => ({
    projects: [] as Project[],
    currentProject: null as Project | null,
    tasks: [] as Task[],
    currentTask: null as Task | null,
    reviews: [] as TaskReview[],
    readiness: null as CoreLoopReadiness | null,
    detectedTestCommand: null as string | null,
    devAiKind: 'codex_cli' as 'codex_cli' | 'claude_cli',
    // Reentrancy guard against duplicate-click starting the loop twice for
    // the same task — mirrors the same pattern `nova.ts`'s `sending` guard
    // uses for chat.
    starting: false,
    lastError: '' as string,
  }),
  getters: {
    reviewerAiKind(state): 'codex_cli' | 'claude_cli' {
      return state.devAiKind === 'codex_cli' ? 'claude_cli' : 'codex_cli'
    },
    // Roles may only be changed before the task has actually started —
    // once it has left `confirmed`/`queued`, the UI must not let the user
    // change the combination out from under an in-flight run.
    canChangeRoles(state): boolean {
      if (!state.currentTask) return true
      return state.currentTask.state === 'draft' || state.currentTask.state === 'awaiting_confirmation'
    },
    isRunning(state): boolean {
      const runningStates: TaskState[] = ['queued', 'running', 'verifying', 'safe_fixing']
      return !!state.currentTask && runningStates.includes(state.currentTask.state)
    },
  },
  actions: {
    async loadProjects() {
      this.projects = await invoke<Project[]>('list_projects')
    },
    async openProject(localPath: string, name: string) {
      this.lastError = ''
      try {
        const project = await invoke<Project>('create_project', {
          request: {
            name,
            local_path: localPath,
            source_type: 'local',
            source_url: null,
            purpose: null,
            long_term_goal: null,
            user_preferences: null,
            prohibitions: null,
          },
        })
        this.currentProject = project
        await this.loadProjects()
        await this.loadTasks()
      } catch (error) {
        this.lastError = String(error)
        throw error
      }
    },
    selectProject(project: Project) {
      this.currentProject = project
      this.currentTask = null
      this.reviews = []
      this.readiness = null
      this.detectedTestCommand = null
    },
    async loadTasks() {
      if (!this.currentProject) return
      this.tasks = await invoke<Task[]>('list_tasks_for_project', { projectId: this.currentProject.id })
    },
    async createTask(fields: {
      title: string
      goal: string
      in_scope: string
      not_in_scope: string
      done_criteria: string
    }) {
      if (!this.currentProject) throw new Error('请先选择项目')
      this.lastError = ''
      try {
        const task = await invoke<Task>('create_task', {
          request: {
            project_id: this.currentProject.id,
            title: fields.title,
            goal: fields.goal || null,
            in_scope: fields.in_scope || null,
            not_in_scope: fields.not_in_scope || null,
            done_criteria: fields.done_criteria || null,
            ai_kind: this.devAiKind,
          },
        })
        this.currentTask = task
        await this.loadTasks()
        await this.detectTestCommand()
      } catch (error) {
        this.lastError = String(error)
        throw error
      }
    },
    selectTask(task: Task) {
      this.currentTask = task
      this.reviews = []
    },
    async detectTestCommand() {
      if (!this.currentProject) return
      this.detectedTestCommand = await invoke<string | null>('detect_test_command', {
        projectId: this.currentProject.id,
      })
    },
    async confirmTestCommand(command: string, source: 'auto' | 'user') {
      if (!this.currentTask) return
      this.currentTask = await invoke<Task>('confirm_task_test_command', {
        taskId: this.currentTask.id,
        command,
        source,
      })
    },
    // Requirement confirmation: draft -> awaiting_confirmation -> confirmed.
    // The backend is the sole authority on whether each hop is legal; this
    // is purely "the user clicked confirm."
    async confirmRequirement() {
      if (!this.currentTask) return
      this.lastError = ''
      try {
        await invoke<Task>('transition_task', {
          taskId: this.currentTask.id,
          toState: 'awaiting_confirmation',
          reason: '用户确认需求',
          correlationId: null,
        })
        this.currentTask = await invoke<Task>('transition_task', {
          taskId: this.currentTask.id,
          toState: 'confirmed',
          reason: '用户确认需求',
          correlationId: null,
        })
      } catch (error) {
        this.lastError = String(error)
        throw error
      }
    },
    async checkReadiness() {
      this.readiness = await invoke<CoreLoopReadiness>('get_core_loop_readiness', {
        devAiKind: this.devAiKind,
      })
    },
    async startTask() {
      if (!this.currentTask || this.starting) return
      this.starting = true
      this.lastError = ''
      try {
        await invoke('start_core_loop', { taskId: this.currentTask.id })
        await this.refreshCurrentTask()
      } catch (error) {
        this.lastError = String(error)
        throw error
      } finally {
        this.starting = false
      }
    },
    async cancelTask() {
      if (!this.currentTask) return
      this.lastError = ''
      try {
        await invoke('cancel_core_loop', { taskId: this.currentTask.id })
      } catch (error) {
        this.lastError = String(error)
        throw error
      }
    },
    async refreshCurrentTask() {
      if (!this.currentTask || !this.currentProject) return
      await this.loadTasks()
      const refreshed = this.tasks.find((t) => t.id === this.currentTask!.id)
      if (refreshed) this.currentTask = refreshed
      await this.loadReviews()
    },
    async loadReviews() {
      if (!this.currentTask || !this.currentProject) return
      this.reviews = await invoke<TaskReview[]>('list_task_reviews', {
        projectId: this.currentProject.id,
        taskId: this.currentTask.id,
      })
    },
    async acceptResult(accepted: boolean, note: string) {
      if (!this.currentTask || !this.currentProject) return
      this.lastError = ''
      try {
        await invoke('record_user_acceptance', {
          projectId: this.currentProject.id,
          taskId: this.currentTask.id,
          accepted,
          userNote: note || null,
        })
        if (accepted) {
          this.currentTask = await invoke<Task>('transition_task', {
            taskId: this.currentTask.id,
            toState: 'completed',
            reason: '用户已验收',
            correlationId: null,
          })
        }
        await this.refreshCurrentTask()
      } catch (error) {
        this.lastError = String(error)
        throw error
      }
    },
  },
})
