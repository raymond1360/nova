import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import type {
  CliBinaryTrustStatus,
  CoreLoopReadiness,
  NoTestsConfirmation,
  Project,
  Task,
  TaskReview,
  TaskState,
} from '../types'

// Phase 3 remediation (Major 1), extended by the second re-review
// remediation: the minimal, real-backend-connected core loop UI's store.
// Deliberately does not attempt the full Phase 4 timeline/evidence center —
// only what a non-technical user needs to select a project via a real
// system folder picker, confirm a requirement, pick a role combination, see
// CLI readiness and trust status, confirm a test command or explicitly
// acknowledge there is none, start/cancel a run, and accept/reject the
// result through one atomic backend call.
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
    // Second re-review remediation: CLI binary configuration + trust.
    codexBinaryPath: '' as string,
    claudeBinaryPath: '' as string,
    codexTrust: null as CliBinaryTrustStatus | null,
    claudeTrust: null as CliBinaryTrustStatus | null,
    restoring: false,
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
    // The round a stalled `needs_user_action` task is stuck waiting on —
    // round 1 unless a round-1 review has already been recorded, in which
    // case it is the round-2 automatic fix pass. Mirrors
    // `core_loop::current_pending_round`'s own logic so the confirmation the
    // UI writes always targets the round the backend is actually stuck on.
    pendingRound(state): number {
      const roundOneReviewed = state.reviews.some(
        (r) => r.result_generation === state.currentTask?.result_generation && r.round === 1,
      )
      return roundOneReviewed ? 2 : 1
    },
    // True only when the task is genuinely stuck for the "no tests
    // detected" reason (as opposed to any other `needs_user_action` cause,
    // e.g. an action-ledger crash-recovery uncertainty) — approximated from
    // the frontend's own view of state, since the exact stall reason lives
    // in `task_state_events`, not on the `Task` row itself: no command has
    // ever been confirmed for the task, and the most recent detection also
    // came back empty.
    noTestsConfirmationNeeded(state): boolean {
      return (
        !!state.currentTask &&
        state.currentTask.state === 'needs_user_action' &&
        !state.currentTask.test_command_redacted &&
        state.detectedTestCommand === null
      )
    },
  },
  actions: {
    async loadProjects() {
      this.projects = await invoke<Project[]>('list_projects')
    },
    // Second re-review remediation (Major 1): a real system folder picker
    // — the only way an ordinary user selects a project directory now.
    // The resulting path is only ever displayed, never hand-typed.
    async pickAndOpenProjectFolder(name: string) {
      const selected = await open({ directory: true, multiple: false })
      if (!selected || Array.isArray(selected)) return
      await this.openProject(selected, name.trim() || selected)
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
        await invoke('set_last_active_project', { projectId: project.id })
        await this.loadProjects()
        await this.loadTasks()
      } catch (error) {
        this.lastError = String(error)
        throw error
      }
    },
    async selectProject(project: Project) {
      this.currentProject = project
      this.currentTask = null
      this.reviews = []
      this.readiness = null
      this.detectedTestCommand = null
      await invoke('set_last_active_project', { projectId: project.id }).catch(() => {})
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
      if (!this.currentTask || !this.currentProject) return
      this.currentTask = await invoke<Task>('confirm_task_test_command', {
        projectId: this.currentProject.id,
        taskId: this.currentTask.id,
        command,
        source,
      })
    },
    // Second re-review remediation (Blocker 3): the only way `no_tests_
    // applicable` evidence can ever be recorded — a real, explicit user
    // action with no default. `reason` is optional free text, never
    // pre-filled with an assumption.
    async confirmNoTestsApplicable(reason: string) {
      if (!this.currentTask || !this.currentProject) return
      this.lastError = ''
      try {
        await invoke<NoTestsConfirmation>('confirm_no_tests_applicable', {
          projectId: this.currentProject.id,
          taskId: this.currentTask.id,
          round: this.pendingRound,
          reason: reason || null,
        })
        await this.refreshCurrentTask()
      } catch (error) {
        this.lastError = String(error)
        throw error
      }
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
    // --- Second re-review remediation: CLI executable trust ---
    async resolveCliBinaryPath(name: string): Promise<string> {
      return invoke<string>('resolve_cli_binary_path', { name })
    },
    async pickCliBinaryFile(): Promise<string | null> {
      const selected = await open({ directory: false, multiple: false })
      if (!selected || Array.isArray(selected)) return null
      return selected
    },
    async loadCliBinaryTrust(kind: 'codex_cli' | 'claude_cli') {
      const configured = kind === 'codex_cli' ? this.codexBinaryPath || 'codex' : this.claudeBinaryPath || 'claude'
      const status = await invoke<CliBinaryTrustStatus>('get_cli_binary_trust_status', {
        kind,
        configuredPath: configured,
      })
      if (kind === 'codex_cli') this.codexTrust = status
      else this.claudeTrust = status
    },
    async confirmCliBinaryPath(kind: 'codex_cli' | 'claude_cli', path: string) {
      this.lastError = ''
      try {
        const status = await invoke<CliBinaryTrustStatus>('confirm_cli_binary_path', { kind, path })
        if (kind === 'codex_cli') {
          this.codexBinaryPath = path
          this.codexTrust = status
        } else {
          this.claudeBinaryPath = path
          this.claudeTrust = status
        }
        await invoke('set_setting', {
          key: kind === 'codex_cli' ? 'codex_binary_path' : 'claude_binary_path',
          value: path,
        })
      } catch (error) {
        this.lastError = String(error)
        throw error
      }
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
      if (this.currentTask?.state === 'needs_user_action') {
        await this.detectTestCommand().catch(() => {})
      }
    },
    async loadReviews() {
      if (!this.currentTask || !this.currentProject) return
      this.reviews = await invoke<TaskReview[]>('list_task_reviews', {
        projectId: this.currentProject.id,
        taskId: this.currentTask.id,
      })
    },
    // Second re-review remediation (Major 5): one atomic backend call —
    // replaces the old two-step `record_user_acceptance` +
    // `transition_task` pattern, which never regenerated the seven
    // documents and could leave a rejected task stuck at
    // `awaiting_user_acceptance` with a contradictory rejection record.
    async acceptResult(accepted: boolean, note: string) {
      if (!this.currentTask || !this.currentProject) return
      this.lastError = ''
      try {
        this.currentTask = await invoke<Task>('accept_task_result', {
          projectId: this.currentProject.id,
          taskId: this.currentTask.id,
          accepted,
          note: note || null,
        })
        await this.refreshCurrentTask()
      } catch (error) {
        this.lastError = String(error)
        throw error
      }
    },
    async resyncDocuments() {
      if (!this.currentTask || !this.currentProject) return
      this.currentTask = await invoke<Task>('resync_task_documents', {
        projectId: this.currentProject.id,
        taskId: this.currentTask.id,
      })
    },
    // Second re-review remediation (UI restart/restore): re-loads the last
    // active project and its current unfinished task (if any) so an app
    // restart does not lose the user's place — polling for an in-flight
    // task resumes automatically via `isRunning` once `currentTask` is set.
    async restoreLastSession() {
      this.restoring = true
      try {
        const project = await invoke<Project | null>('get_last_active_project')
        if (!project) return
        this.currentProject = project
        await this.loadTasks()
        const unfinished = this.tasks.find((t) =>
          (['draft', 'awaiting_confirmation', 'confirmed', 'queued', 'running', 'verifying', 'safe_fixing', 'preview_ready', 'awaiting_user_acceptance', 'needs_user_action'] as TaskState[]).includes(
            t.state,
          ),
        )
        if (unfinished) {
          this.currentTask = unfinished
          await this.loadReviews()
        }
      } catch (error) {
        this.lastError = String(error)
      } finally {
        this.restoring = false
      }
    },
  },
})
