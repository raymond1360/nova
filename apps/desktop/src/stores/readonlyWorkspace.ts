import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import type { Project, Task, TaskEvidenceOverview } from '../types'

export const DRAFT_PREFIX = 'nova.ui-ro1.draft/1/'
// Transport bookkeeping is not a second task state machine. A stale request must
// finish before another overview request starts, even when the selection changes.
const transports = new WeakMap<object, { busy: boolean; queued: boolean }>()
function transport(store: object) {
  let value = transports.get(store)
  if (!value) { value = { busy: false, queued: false }; transports.set(store, value) }
  return value
}
const draftKey = (project: string, task: string) => DRAFT_PREFIX + JSON.stringify([project, task])

export const useReadonlyWorkspace = defineStore('readonlyWorkspace', {
  state: () => ({
    active: false, epoch: 0, projectRequest: 0, taskRequest: 0, overviewRequest: 0,
    projects: [] as Project[], tasks: [] as Task[], projectId: '', taskId: '',
    overview: null as TaskEvidenceOverview | null,
    projectsLoading: false, tasksLoading: false, overviewLoading: false,
    projectsError: '', tasksError: '', overviewError: '', lastSuccessfulReadAt: null as string | null,
    drafts: {} as Record<string, string>, cacheErrors: {} as Record<string, boolean>,
  }),
  getters: {
    draft(state): string { return state.drafts[draftKey(state.projectId, state.taskId)] ?? '' },
    cacheFailed(state): boolean { return !!state.cacheErrors[draftKey(state.projectId, state.taskId)] },
  },
  actions: {
    async activate() {
      this.active = true; this.epoch++; this.projectId = ''; this.taskId = ''
      this.projects = []; this.tasks = []; this.tasksError = ''; this.tasksLoading = false
      this.clearOverview(); await this.loadProjects()
    },
    dispose() {
      this.active = false; this.epoch++; this.projectRequest++; this.taskRequest++; this.overviewRequest++
      this.projectsLoading = false; this.tasksLoading = false; this.clearOverview()
      transport(this).queued = false
    },
    clearOverview() {
      this.overviewRequest++; this.overview = null; this.overviewError = ''
      this.overviewLoading = false; this.lastSuccessfulReadAt = null
    },
    async loadProjects() {
      if (!this.active) return
      const token = ++this.projectRequest
      const current = () => this.active && this.projectRequest === token
      this.projectsLoading = true; this.projectsError = ''
      try {
        const rows = await invoke<Project[]>('list_projects')
        if (current()) this.projects = rows
      } catch { if (current()) this.projectsError = 'READ_FAILED' }
      finally { if (current()) this.projectsLoading = false }
    },
    async selectProject(id: string) {
      if (!this.active || (id && !this.projects.some(p => p.id === id))) return
      this.epoch++; this.projectId = id; this.taskId = ''; this.tasks = []
      this.taskRequest++; this.tasksLoading = false; this.tasksError = ''; this.clearOverview()
      this.restoreDraft(); if (!id) return
      const epoch = this.epoch, token = ++this.taskRequest
      const current = () => this.active && this.epoch === epoch && this.projectId === id && this.taskRequest === token
      this.tasksLoading = true
      try {
        const rows = await invoke<Task[]>('list_tasks_for_project', { projectId: id })
        if (rows.some(t => t.project_id !== id)) throw new Error('OWNERSHIP')
        if (current()) this.tasks = rows
      } catch { if (current()) this.tasksError = 'READ_FAILED' }
      finally { if (current()) this.tasksLoading = false }
    },
    selectTask(id: string) {
      if (!this.active || (id && !this.tasks.some(t => t.id === id && t.project_id === this.projectId))) return
      this.epoch++; this.taskId = id; this.clearOverview(); this.restoreDraft()
      void this.refreshOverview()
    },
    async refreshOverview() {
      if (!this.active || !this.projectId || !this.taskId) return
      const wire = transport(this)
      if (wire.busy) { wire.queued = true; return }
      wire.busy = true; wire.queued = false
      const epoch = this.epoch, projectId = this.projectId, taskId = this.taskId, token = ++this.overviewRequest
      const current = () => this.active && this.epoch === epoch && this.projectId === projectId && this.taskId === taskId && this.overviewRequest === token
      this.overviewLoading = true
      try {
        const value = await invoke<TaskEvidenceOverview>('get_task_evidence_overview', { projectId, taskId })
        if (value.project_id !== projectId || value.task_id !== taskId) throw new Error('OWNERSHIP')
        if (current()) { this.overview = value; this.overviewError = ''; this.lastSuccessfulReadAt = new Date().toISOString() }
      } catch { if (current()) this.overviewError = 'READ_FAILED' }
      finally {
        if (current()) this.overviewLoading = false
        wire.busy = false
        if (wire.queued && this.active) { wire.queued = false; void this.refreshOverview() }
      }
    },
    restoreDraft() {
      const key = draftKey(this.projectId, this.taskId)
      if (Object.prototype.hasOwnProperty.call(this.drafts, key)) return
      try { this.drafts[key] = sessionStorage.getItem(key) ?? ''; this.cacheErrors[key] = false }
      catch { this.drafts[key] = ''; this.cacheErrors[key] = true }
    },
    editDraft(text: string) {
      if (!this.projectId) return
      const key = draftKey(this.projectId, this.taskId)
      this.drafts[key] = text
      try { sessionStorage.setItem(key, text); this.cacheErrors[key] = false }
      catch { this.cacheErrors[key] = true }
    },
  },
})
