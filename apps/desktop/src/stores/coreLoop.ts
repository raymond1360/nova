import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import type {
  CliBinaryTrustStatus,
  CoreLoopReadiness,
  NoBuildConfirmation,
  NoTestsConfirmation,
  Project,
  Task,
  TaskDocumentContent,
  TaskDocumentInfo,
  TaskEvidenceOverview,
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
    // Work Package 3 final review remediation (Major 1): whether the
    // backend has confirmed the current task is genuinely stalled on "no
    // detectable test command, awaiting an explicit no-tests confirmation"
    // — populated from the real, authoritative `get_no_tests_confirmation_
    // needed` backend query, never guessed on the frontend from local
    // heuristics (the old `test_command_redacted`/`detectedTestCommand`
    // approximation could show/hide the card based on stale or approximate
    // state — the command itself must be the final authority, exactly like
    // `noBuildConfirmationNeeded` already is for the no-build case).
    noTestsConfirmationNeeded: false as boolean,
    // Work Package 3 final review remediation (Major 1): mirrors
    // `noBuildConfirmationRequestToken` exactly — a monotonically
    // increasing token guarding `loadNoTestsConfirmationNeeded` against a
    // stale response from a prior project/task/state applying after the
    // user has since navigated away.
    noTestsConfirmationRequestToken: 0 as number,
    // Work Package 3 final review remediation (Major 1): mirrors
    // `confirmingNoBuild` exactly — in-flight guard for
    // `confirmNoTestsApplicable` so a duplicate click (or any other
    // duplicate trigger) can never fire a second `invoke`.
    confirmingNoTests: false as boolean,
    // v0.1.0 scope correction (Work Package 2): whether the backend has
    // confirmed the current task is genuinely stalled on build detection
    // being unable to determine whether a build applies — populated from
    // the real, authoritative `get_no_build_confirmation_needed` backend
    // query (never guessed on the frontend), mirroring the same pattern
    // `noTestsConfirmationNeeded` already uses for the no-tests case.
    noBuildConfirmationNeeded: false as boolean,
    // v0.1.0 scope correction (Work Package 2 remediation, Major 1): a
    // monotonically increasing token identifying "the current no-build
    // query generation." `loadNoBuildConfirmationNeeded` captures this
    // (plus the exact project/task id it is querying for) before awaiting
    // the backend; bumped by every navigation action that could make an
    // in-flight query's eventual answer stale (selecting a different
    // task, switching project, or the current task leaving
    // `needs_user_action`) and by every new query itself. A response is
    // only ever applied if the token, project id, task id, and live task
    // state all still match what was captured — otherwise it is a stale
    // answer for a task/project the user has since navigated away from,
    // and is silently discarded.
    noBuildConfirmationRequestToken: 0 as number,
    // v0.1.0 scope correction (Work Package 2 remediation, Minor 1):
    // in-flight guard for `confirmNoBuildApplicable` — mirrors the
    // `starting` guard `startTask` already uses. Set for the duration of
    // exactly one backend call; the view layer disables the confirm
    // button while this is `true` so a double click (or any other
    // duplicate trigger) can never fire a second `invoke`.
    confirmingNoBuild: false as boolean,
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
    // Codex re-review remediation (Major 1): the backend is the sole
    // source of truth for which round a stalled task is stuck on — this is
    // never computed from `reviews` on the frontend. Populated by
    // `loadPendingRound()`, called from `refreshCurrentTask()`. Defaults to
    // 1, matching the backend's own default before anything has run.
    pendingRound: 1 as number,
    // Phase 4 (evidence and document gate): the real, backend-derived
    // timeline/evidence/completion-gate view for the current task —
    // always a full replace (never appended to), so a repeated load can
    // never duplicate a timeline entry; see `loadEvidenceOverview`.
    evidenceOverview: null as TaskEvidenceOverview | null,
    // Mirrors `noTestsConfirmationRequestToken`'s stale-response-guard
    // pattern exactly: bumped by every navigation action that could make
    // an in-flight query's eventual answer stale, and by every new query
    // itself. A response is only ever applied if the token, project id,
    // and task id all still match what was captured when the request was
    // issued — unlike the no-tests/no-build queries, this one is not also
    // gated on task state, since evidence is relevant in every state.
    evidenceRequestToken: 0 as number,
    taskDocuments: [] as TaskDocumentInfo[],
    documentsRequestToken: 0 as number,
    // The currently-expanded document's redacted content, if the user has
    // asked to view one — cleared on every task/project switch so a slow
    // response for a previously-viewed document can never render under a
    // different task.
    selectedDocument: null as TaskDocumentContent | null,
    selectedDocumentRequestToken: 0 as number,
    loadingDocument: false,
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
    // Codex re-review remediation (Blocker 1): the v0.1.0 start gate reads
    // *only* the selected developer AI's own readiness — the other AI's
    // `review_ready` (always a diagnostic-only, possibly-`false` value; see
    // `get_core_loop_readiness`) must never participate in this decision.
    // `starting` is intentionally not included here — that guard is applied
    // separately by callers (`handleStart`'s disabled binding also checks
    // `store.starting`), so `canStart` reflects purely "is the selected dev
    // AI ready," independent of "is a start already in flight."
    canStart(state): boolean {
      return !!state.readiness?.dev_ready
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
      this.noBuildConfirmationNeeded = false
      this.noTestsConfirmationNeeded = false
      this.evidenceOverview = null
      this.taskDocuments = []
      this.selectedDocument = null
      // v0.1.0 scope correction (Work Package 2 remediation, Major 1) /
      // Work Package 3 final review remediation (Major 1): invalidates any
      // in-flight no-build/no-tests query from whatever project was
      // previously selected — its eventual answer, however it resolves,
      // can never apply to this new project/task pair.
      this.noBuildConfirmationRequestToken += 1
      this.noTestsConfirmationRequestToken += 1
      // Phase 4: same stale-response invalidation for the evidence/document
      // queries below.
      this.evidenceRequestToken += 1
      this.documentsRequestToken += 1
      this.selectedDocumentRequestToken += 1
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
      this.noBuildConfirmationNeeded = false
      this.noTestsConfirmationNeeded = false
      this.evidenceOverview = null
      this.taskDocuments = []
      this.selectedDocument = null
      // v0.1.0 scope correction (Work Package 2 remediation, Major 1) /
      // Work Package 3 final review remediation (Major 1): invalidates any
      // no-build/no-tests query still in flight for whatever task was
      // previously selected, *before* possibly starting a new one below —
      // a slow response for task A must never be applied once the user has
      // moved on to task B, even if B itself never triggers a new query
      // (e.g. B is not `needs_user_action`).
      this.noBuildConfirmationRequestToken += 1
      this.noTestsConfirmationRequestToken += 1
      this.evidenceRequestToken += 1
      this.documentsRequestToken += 1
      this.selectedDocumentRequestToken += 1
      // Both require a real backend query — selecting a task that is
      // already sitting at `needs_user_action` (e.g. after reopening the
      // project) must still surface the right prompt without waiting for
      // the next poll/refresh. Deliberately not awaited: `selectTask`
      // itself stays synchronous for its existing callers, and a failed
      // query here just leaves the prompt hidden rather than blocking task
      // selection.
      if (task.state === 'needs_user_action') {
        this.loadNoBuildConfirmationNeeded().catch(() => {})
        this.loadNoTestsConfirmationNeeded().catch(() => {})
      }
      // Phase 4: unlike the no-build/no-tests prompts, evidence and
      // documents are relevant regardless of task state — always load them
      // for whichever task the user just selected.
      this.loadEvidenceOverview().catch(() => {})
      this.loadTaskDocuments().catch(() => {})
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
    //
    // Work Package 3 final review remediation (Major 1): deliberately
    // never sends `round` (or any generation) — `confirm_no_tests_
    // applicable` on the backend derives the generation/round itself from
    // the task's own real state, exactly like `confirmNoBuildApplicable`
    // already trusts the backend for the state/action/evidence gate. Also
    // re-entrant: if a prior attempt already wrote the confirmation but the
    // task did not yet resume (a half-success crash), calling this again
    // still completes the recovery — the frontend never needs to
    // distinguish "first call" from "retry".
    //
    // Mirrors `confirmNoBuildApplicable`'s two safeguards exactly: first,
    // `confirmingNoTests` makes a second overlapping call (e.g. a double
    // click) a pure no-op — it never fires a second `invoke`. Second, the
    // project/task this call was actually submitted for is captured up
    // front; if the user has since selected a different task or project by
    // the time the backend responds, this call's outcome — success or
    // failure — is applied only where doing so is still meaningful, never
    // onto whatever the user has since navigated to.
    async confirmNoTestsApplicable(reason: string) {
      if (!this.currentTask || !this.currentProject || this.confirmingNoTests) return
      this.lastError = ''
      this.confirmingNoTests = true
      const projectId = this.currentProject.id
      const taskId = this.currentTask.id
      try {
        await invoke<NoTestsConfirmation>('confirm_no_tests_applicable', {
          projectId,
          taskId,
          reason: reason || null,
        })
        if (this.currentProject?.id === projectId && this.currentTask?.id === taskId) {
          await this.refreshCurrentTask()
        }
      } catch (error) {
        if (this.currentProject?.id === projectId && this.currentTask?.id === taskId) {
          this.lastError = String(error)
        }
        throw error
      } finally {
        this.confirmingNoTests = false
      }
    },
    // Work Package 3 final review remediation (Major 1 / Suggestion 2): the
    // real, backend-authoritative check for whether the current task is
    // genuinely stalled on "no detectable test command, awaiting an
    // explicit no-tests confirmation" — never inferred from any
    // frontend-only signal. Safe to call whenever a task is
    // `needs_user_action`; returns `false` for every other reason a task
    // can be stuck there. Mirrors `loadNoBuildConfirmationNeeded`'s
    // stale-response guard exactly: the result is only ever written if, at
    // resolution time, the token is still the latest one issued, the
    // selected project and task are still the ones this call started for,
    // and the task is still genuinely `needs_user_action`.
    async loadNoTestsConfirmationNeeded() {
      if (!this.currentTask || !this.currentProject) {
        this.noTestsConfirmationNeeded = false
        return
      }
      this.noTestsConfirmationRequestToken += 1
      const requestToken = this.noTestsConfirmationRequestToken
      const projectId = this.currentProject.id
      const taskId = this.currentTask.id
      const needed = await invoke<boolean>('get_no_tests_confirmation_needed', {
        projectId,
        taskId,
      })
      if (
        this.noTestsConfirmationRequestToken === requestToken &&
        this.currentProject?.id === projectId &&
        this.currentTask?.id === taskId &&
        this.currentTask?.state === 'needs_user_action'
      ) {
        this.noTestsConfirmationNeeded = needed
      }
    },
    // v0.1.0 scope correction (Work Package 2): the real, backend-
    // authoritative check for whether the current task is genuinely
    // stalled on "cannot determine whether this project needs a build" —
    // never inferred from any frontend-only signal. Safe to call whenever
    // a task is `needs_user_action`; returns `false` for every other
    // reason a task can be stuck there.
    //
    // v0.1.0 scope correction (Work Package 2 remediation, Major 1): this
    // query is asynchronous and the user can select a different task (or
    // project, or the current task can leave `needs_user_action`) before
    // it resolves. A stale response applying to the wrong task would show
    // task A's "confirm no build" prompt while the user is looking at task
    // B. Guarded with a captured request token plus the exact
    // project/task id this call is *for*: the result is only ever written
    // if, at resolution time, the token is still the latest one issued,
    // the selected project and task are still the ones this call started
    // for, and the task is still genuinely `needs_user_action`. A stale
    // response — success or failure — is silently discarded and never
    // touches store state, never sets `lastError`, and never displays
    // anything for whichever task is now actually selected.
    async loadNoBuildConfirmationNeeded() {
      if (!this.currentTask || !this.currentProject) {
        this.noBuildConfirmationNeeded = false
        return
      }
      this.noBuildConfirmationRequestToken += 1
      const requestToken = this.noBuildConfirmationRequestToken
      const projectId = this.currentProject.id
      const taskId = this.currentTask.id
      const needed = await invoke<boolean>('get_no_build_confirmation_needed', {
        projectId,
        taskId,
      })
      if (
        this.noBuildConfirmationRequestToken === requestToken &&
        this.currentProject?.id === projectId &&
        this.currentTask?.id === taskId &&
        this.currentTask?.state === 'needs_user_action'
      ) {
        this.noBuildConfirmationNeeded = needed
      }
    },
    // v0.1.0 scope correction (Work Package 2): the only way a no-build
    // confirmation can ever be recorded — a real, explicit user action.
    // Deliberately never sends a `round`: `confirm_no_build_applicable` on
    // the backend derives the generation/round itself from the task's own
    // real state, exactly like `confirmNoTestsApplicable` already trusts
    // the backend for the state/action/evidence gate. Also re-entrant: if
    // a prior attempt already wrote the confirmation but the task did not
    // yet resume (a half-success crash), calling this again still
    // completes the recovery — the frontend never needs to distinguish
    // "first call" from "retry".
    //
    // v0.1.0 scope correction (Work Package 2 remediation, Minor 1): two
    // safeguards on top of that. First, `confirmingNoBuild` makes a second
    // overlapping call (e.g. a double click) a pure no-op — it never fires
    // a second `invoke`, so the backend never has to be relied upon alone
    // to dedupe a rapid double-submit. Second, the project/task this call
    // was actually submitted for is captured up front; if the user has
    // since selected a different task or project by the time the backend
    // responds, this call's outcome — success or failure — is applied to
    // its *own* captured state only where doing so is still meaningful
    // (`refreshCurrentTask`/`lastError` are skipped entirely once stale),
    // never onto whatever the user has since navigated to.
    async confirmNoBuildApplicable(reason: string) {
      if (!this.currentTask || !this.currentProject || this.confirmingNoBuild) return
      this.lastError = ''
      this.confirmingNoBuild = true
      const projectId = this.currentProject.id
      const taskId = this.currentTask.id
      try {
        await invoke<NoBuildConfirmation>('confirm_no_build_applicable', {
          projectId,
          taskId,
          reason: reason || null,
        })
        if (this.currentProject?.id === projectId && this.currentTask?.id === taskId) {
          await this.refreshCurrentTask()
        }
      } catch (error) {
        if (this.currentProject?.id === projectId && this.currentTask?.id === taskId) {
          this.lastError = String(error)
        }
        throw error
      } finally {
        this.confirmingNoBuild = false
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
    // Codex re-review remediation (Blocker 1): the actual v0.1.0 start
    // decision, as a single store action the view layer calls directly —
    // re-checks readiness, then gates purely on `canStart` (the selected
    // dev AI's own readiness; see that getter's own comment for why the
    // other AI's `review_ready` never participates). Returns whether a
    // start was actually attempted, so the caller knows whether to begin
    // polling.
    async startIfReady(): Promise<boolean> {
      await this.checkReadiness()
      if (!this.canStart) return false
      await this.startTask()
      return true
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
    // Codex re-review remediation (Major 1): the backend's own Action
    // Ledger-derived pending round — never inferred from `reviews` here.
    async loadPendingRound() {
      if (!this.currentTask || !this.currentProject) return
      this.pendingRound = await invoke<number>('get_pending_round', {
        projectId: this.currentProject.id,
        taskId: this.currentTask.id,
      })
    },
    async refreshCurrentTask() {
      if (!this.currentTask || !this.currentProject) return
      await this.loadTasks()
      const refreshed = this.tasks.find((t) => t.id === this.currentTask!.id)
      if (refreshed) this.currentTask = refreshed
      await this.loadReviews()
      await this.loadPendingRound().catch(() => {})
      // Phase 4: every refresh re-derives the timeline/evidence/documents
      // from the backend's current truth — never appended to, so polling
      // while a task is running can never duplicate a timeline entry.
      await this.loadEvidenceOverview().catch(() => {})
      await this.loadTaskDocuments().catch(() => {})
      if (this.currentTask?.state === 'needs_user_action') {
        await this.detectTestCommand().catch(() => {})
        await this.loadNoBuildConfirmationNeeded().catch(() => {})
        await this.loadNoTestsConfirmationNeeded().catch(() => {})
      } else {
        this.noBuildConfirmationNeeded = false
        this.noTestsConfirmationNeeded = false
        // v0.1.0 scope correction (Work Package 2 remediation, Major 1) /
        // Work Package 3 final review remediation (Major 1): the task just
        // left `needs_user_action` (or there is no current task) —
        // invalidate any no-build/no-tests query still in flight for it so
        // a late response cannot resurrect the prompt.
        this.noBuildConfirmationRequestToken += 1
        this.noTestsConfirmationRequestToken += 1
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
    // Phase 4 (evidence and document gate): the one real, backend-derived
    // timeline + evidence summary + completion-gate view — never inferred
    // or assembled from other frontend state. Mirrors
    // `loadNoTestsConfirmationNeeded`'s stale-response guard: the result is
    // only ever written if, at resolution time, the token is still the
    // latest one issued and the selected project/task are still the ones
    // this call started for. Always a full replace of `evidenceOverview`,
    // never appended to, so a repeated call (e.g. the running-task poll)
    // can never duplicate a timeline entry.
    async loadEvidenceOverview() {
      if (!this.currentTask || !this.currentProject) {
        this.evidenceOverview = null
        return
      }
      this.evidenceRequestToken += 1
      const requestToken = this.evidenceRequestToken
      const projectId = this.currentProject.id
      const taskId = this.currentTask.id
      const overview = await invoke<TaskEvidenceOverview>('get_task_evidence_overview', {
        projectId,
        taskId,
      })
      if (
        this.evidenceRequestToken === requestToken &&
        this.currentProject?.id === projectId &&
        this.currentTask?.id === taskId
      ) {
        this.evidenceOverview = overview
      }
    },
    // Phase 4: lists the seven result documents' display metadata (never
    // their content — see `viewTaskDocument` for that). Same
    // stale-response guard as `loadEvidenceOverview`.
    async loadTaskDocuments() {
      if (!this.currentTask || !this.currentProject) {
        this.taskDocuments = []
        return
      }
      this.documentsRequestToken += 1
      const requestToken = this.documentsRequestToken
      const projectId = this.currentProject.id
      const taskId = this.currentTask.id
      const docs = await invoke<TaskDocumentInfo[]>('list_task_documents', {
        projectId,
        taskId,
      })
      if (
        this.documentsRequestToken === requestToken &&
        this.currentProject?.id === projectId &&
        this.currentTask?.id === taskId
      ) {
        this.taskDocuments = docs
      }
    },
    // Phase 4: reads exactly one document's redacted content, identified
    // only by its fixed `document_type` tag — never a caller-supplied
    // path. A stale response (the user has since switched task/project) is
    // silently discarded, exactly like every other query in this store.
    async viewTaskDocument(documentType: string) {
      if (!this.currentTask || !this.currentProject) return
      this.selectedDocumentRequestToken += 1
      const requestToken = this.selectedDocumentRequestToken
      const projectId = this.currentProject.id
      const taskId = this.currentTask.id
      this.loadingDocument = true
      try {
        const content = await invoke<TaskDocumentContent>('read_task_document', {
          projectId,
          taskId,
          documentType,
        })
        if (
          this.selectedDocumentRequestToken === requestToken &&
          this.currentProject?.id === projectId &&
          this.currentTask?.id === taskId
        ) {
          this.selectedDocument = content
        }
      } finally {
        if (this.selectedDocumentRequestToken === requestToken) {
          this.loadingDocument = false
        }
      }
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
          if (unfinished.state === 'needs_user_action') {
            await this.loadNoBuildConfirmationNeeded().catch(() => {})
            await this.loadNoTestsConfirmationNeeded().catch(() => {})
          }
          // Phase 4: a restored session must surface the same trusted
          // evidence/document view an app restart should never hide.
          await this.loadEvidenceOverview().catch(() => {})
          await this.loadTaskDocuments().catch(() => {})
        }
      } catch (error) {
        this.lastError = String(error)
      } finally {
        this.restoring = false
      }
    },
  },
})
