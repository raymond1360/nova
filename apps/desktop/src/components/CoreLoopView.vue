<script setup lang="ts">
// Phase 3 remediation (Major 1), extended by the second re-review
// remediation: the minimal Open-Local-Project Dual-AI Core Loop UI. Every
// action here calls a real Tauri command through `useCoreLoopStore` —
// nothing here is a static demo. Deliberately does NOT implement: the full
// Phase 4 timeline, the evidence center, GitHub import, Create Product,
// auto-deployment, or push/PR — those are out of this round's scope.
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useCoreLoopStore } from '../stores/coreLoop'
import type { Project, TimelineEntry } from '../types'

// WP2A: labels for the self-check's own `self_reported_status` field —
// a distinct vocabulary from `statusLabels` below (that one is evidence/
// action-ledger statuses, this one is the developer AI's own self-report).
const selfReportedStatusLabels: Record<string, string> = {
  self_reported_complete: '自评已完成',
  self_reported_incomplete: '自评未完成',
  self_reported_blocked: '自评被阻断',
}
function parseJsonStringArray(raw: string | null): string[] {
  if (!raw) return []
  try {
    const parsed = JSON.parse(raw)
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === 'string') : []
  } catch {
    return []
  }
}

const store = useCoreLoopStore()

const newProjectName = ref('')
const taskTitle = ref('')
const taskGoal = ref('')
const taskInScope = ref('')
const taskNotInScope = ref('')
const taskDoneCriteria = ref('')
const testCommandDraft = ref('')
const acceptNote = ref('')
const noTestsReason = ref('')
// No default selection: the user must actively pick one of the two
// choices below — neither radio starts checked.
const noTestsChoice = ref<'' | 'confirm' | 'provide-command'>('')
// v0.1.0 scope correction (Work Package 2): mirrors `noTestsReason` for
// the "confirm this project currently needs no build" prompt.
const noBuildReason = ref('')

const codexPathDraft = ref('codex')
const claudePathDraft = ref('claude')

let pollHandle: ReturnType<typeof setInterval> | null = null

function startPolling() {
  stopPolling()
  pollHandle = setInterval(() => {
    if (store.isRunning) {
      store.refreshCurrentTask().catch(() => {})
    }
  }, 3000)
}
// Timer cleanup: stopped both on unmount and whenever the task leaves a
// running state (the interval body itself is a no-op then, but this also
// releases the underlying OS timer instead of leaving it ticking forever).
function stopPolling() {
  if (pollHandle) {
    clearInterval(pollHandle)
    pollHandle = null
  }
}

onMounted(async () => {
  // UI restart/restore: re-load the last active project and its unfinished
  // task (if any) so an app restart does not lose the user's place.
  await store.restoreLastSession()
  startPolling()
})
onUnmounted(stopPolling)

async function handlePickFolder() {
  await store.pickAndOpenProjectFolder(newProjectName.value)
  newProjectName.value = ''
}

async function handleSelectProject(project: Project) {
  await store.selectProject(project)
  await store.loadTasks()
}

async function handleCreateTask() {
  if (!taskTitle.value.trim()) return
  await store.createTask({
    title: taskTitle.value.trim(),
    goal: taskGoal.value.trim(),
    in_scope: taskInScope.value.trim(),
    not_in_scope: taskNotInScope.value.trim(),
    done_criteria: taskDoneCriteria.value.trim(),
  })
  taskTitle.value = ''
  taskGoal.value = ''
  taskInScope.value = ''
  taskNotInScope.value = ''
  taskDoneCriteria.value = ''
  testCommandDraft.value = store.detectedTestCommand ?? ''
}

async function handleConfirmTestCommand(source: 'auto' | 'user') {
  const command = source === 'auto' ? store.detectedTestCommand ?? '' : testCommandDraft.value.trim()
  if (!command) return
  await store.confirmTestCommand(command, source)
}

async function handleConfirmNoTests() {
  try {
    await store.confirmNoTestsApplicable(noTestsReason.value.trim())
    noTestsReason.value = ''
    noTestsChoice.value = ''
  } catch {
    // `lastError` is already set by the store (unless this call turned
    // out to be stale, in which case nothing was touched at all) — keep
    // the user's draft reason so a retry never requires retyping it.
  }
}

async function handleConfirmNoBuild() {
  try {
    await store.confirmNoBuildApplicable(noBuildReason.value.trim())
    noBuildReason.value = ''
  } catch {
    // `lastError` is already set by the store (unless this call turned
    // out to be stale, in which case nothing was touched at all) — keep
    // the user's draft reason so a retry never requires retyping it.
  }
}

async function handleStart() {
  // Codex re-review remediation (Blocker 1): gates purely on the selected
  // developer AI's own readiness (`store.canStart`) — the other AI's
  // `review_ready` must never block starting a v0.1.0 single-AI task.
  const started = await store.startIfReady()
  if (started) startPolling()
}

async function handleLoadCliTrust() {
  await store.loadCliBinaryTrust('codex_cli')
  await store.loadCliBinaryTrust('claude_cli')
}

async function handleConfirmCodexPath() {
  const resolved = await store.resolveCliBinaryPath(codexPathDraft.value.trim())
  await store.confirmCliBinaryPath('codex_cli', resolved)
}
async function handleConfirmClaudePath() {
  const resolved = await store.resolveCliBinaryPath(claudePathDraft.value.trim())
  await store.confirmCliBinaryPath('claude_cli', resolved)
}
async function handlePickCodexFile() {
  const picked = await store.pickCliBinaryFile()
  if (picked) {
    codexPathDraft.value = picked
    await store.confirmCliBinaryPath('codex_cli', picked)
  }
}
async function handlePickClaudeFile() {
  const picked = await store.pickCliBinaryFile()
  if (picked) {
    claudePathDraft.value = picked
    await store.confirmCliBinaryPath('claude_cli', picked)
  }
}

const stateLabels: Record<string, string> = {
  draft: '草稿',
  awaiting_confirmation: '待确认需求',
  confirmed: '已确认，待开始',
  queued: '排队中',
  running: '开发中',
  verifying: '测试与审核中',
  safe_fixing: '自动修复中',
  preview_ready: '结果已就绪',
  awaiting_user_acceptance: '等待用户验收',
  completed: '已完成',
  needs_user_action: '需要用户处理',
  interrupted: '已中断',
  failed: '失败',
  cancelled: '已取消',
}

// Phase 4 (evidence and document gate): a single shared vocabulary for
// every status string the backend can hand back — evidence outcomes
// (passed/failed/cancelled/timed_out/insufficient_evidence/not_applicable/
// not_run), action-ledger states (planned/started/succeeded/uncertain),
// and the handful of fixed labels this module itself assigns
// (created/confirmed/accepted/rejected). Falls back to `stateLabels` (task
// states reused by `state_transition` timeline entries) and finally to the
// raw string itself — never silently blank.
const statusLabels: Record<string, string> = {
  passed: '通过',
  failed: '失败',
  cancelled: '已取消',
  timed_out: '超时',
  insufficient_evidence: '证据不足',
  not_applicable: '不适用（已确认）',
  not_run: '尚未运行',
  planned: '已计划',
  started: '进行中',
  succeeded: '已完成',
  uncertain: '状态不确定（需用户确认）',
  confirmed: '已确认',
  accepted: '已验收',
  rejected: '未通过验收',
  created: '已创建',
}
function statusLabel(status: string): string {
  return statusLabels[status] ?? stateLabels[status] ?? status
}
const badStatuses = new Set(['failed', 'cancelled', 'timed_out', 'insufficient_evidence', 'uncertain', 'rejected'])
const okStatuses = new Set(['passed', 'succeeded', 'not_applicable', 'confirmed', 'accepted'])
function timelineEntryClass(entry: TimelineEntry): string {
  if (entry.needs_user_action || badStatuses.has(entry.status)) return 'bad'
  if (okStatuses.has(entry.status)) return 'ok'
  return ''
}

const latestTimelineEntry = computed(() => {
  const timeline = store.evidenceOverview?.timeline
  return timeline && timeline.length > 0 ? timeline[timeline.length - 1] : null
})
const latestGitEvent = computed(() => {
  const events = store.evidenceOverview?.git_events
  return events && events.length > 0 ? events[events.length - 1] : null
})
const currentTests = computed(() =>
  (store.evidenceOverview?.tests ?? []).filter((t) => t.is_current_generation && t.is_current_attempt),
)
const currentBuilds = computed(() =>
  (store.evidenceOverview?.builds ?? []).filter((b) => b.is_current_generation && b.is_current_attempt),
)
const currentTestStatusLabel = computed(() => {
  const list = currentTests.value
  return list.length > 0 ? statusLabel(list[list.length - 1].status) : '尚未运行'
})
const currentBuildStatusLabel = computed(() => {
  const list = currentBuilds.value
  return list.length > 0 ? statusLabel(list[list.length - 1].status) : '尚未运行'
})

// WP2A: `store.selfChecks` is oldest-first and append-only (see
// `Database::list_self_checks_for_task`'s own doc comment) — the last
// element is always the most recently recorded self-check, which is what
// this display cares about. Never used for any gate decision here, purely
// for showing the developer AI's own self-assessment to the user.
const latestSelfCheck = computed(() => {
  const list = store.selfChecks
  return list.length > 0 ? list[list.length - 1] : null
})
const latestSelfCheckUnfinishedItems = computed(() => parseJsonStringArray(latestSelfCheck.value?.unfinished_items_json ?? null))
const latestSelfCheckBlockingIssues = computed(() => parseJsonStringArray(latestSelfCheck.value?.blocking_issues_json ?? null))
const latestSelfCheckCheckResults = computed(() => parseJsonStringArray(latestSelfCheck.value?.check_results_json ?? null))

async function handleViewDocument(documentType: string) {
  await store.viewTaskDocument(documentType)
}
</script>

<template>
  <div class="core-loop-view">
    <h2>项目与任务（Phase 3 最小闭环）</h2>
    <p v-if="store.restoring" class="hint">正在恢复上次会话…</p>
    <p v-if="store.lastError" class="error-banner">{{ store.lastError }}</p>

    <section class="block">
      <h3>1. 选择或打开本地项目</h3>
      <div class="row">
        <input v-model="newProjectName" placeholder="项目名称（可选）" />
        <button @click="handlePickFolder">选择文件夹…</button>
      </div>
      <p v-if="store.currentProject" class="hint">
        当前项目路径（只读）：<input class="readonly-path" :value="store.currentProject.local_path" readonly />
      </p>
      <button class="link-button" @click="store.loadProjects">刷新项目列表</button>
      <ul class="project-list">
        <li v-for="project in store.projects" :key="project.id">
          <button :class="{ active: store.currentProject?.id === project.id }" @click="handleSelectProject(project)">
            {{ project.name }} — {{ project.local_path }}
          </button>
        </li>
      </ul>
    </section>

    <section class="block">
      <h3>CLI 配置与信任确认</h3>
      <p class="hint">
        只需配置你选择使用的开发 AI 对应的 CLI；未选用的另一个 CLI 未安装或未登录不影响任务启动。
      </p>
      <button class="link-button" @click="handleLoadCliTrust">查看当前 Codex / Claude CLI 信任状态</button>
      <div class="cli-trust-grid">
        <div class="cli-trust-item">
          <p><strong>Codex CLI</strong></p>
          <div class="row">
            <input v-model="codexPathDraft" placeholder="codex 或完整路径" />
            <button @click="handleConfirmCodexPath">解析并确认</button>
            <button @click="handlePickCodexFile">选择文件…</button>
          </div>
          <p v-if="store.codexTrust">
            <span :class="store.codexTrust.trusted ? 'ok' : 'bad'">{{ store.codexTrust.trusted ? '已信任' : '未信任' }}</span>
            — 解析路径：<code>{{ store.codexTrust.canonical_path }}</code>
            <span v-if="store.codexTrust.mismatch_reason"> — {{ store.codexTrust.mismatch_reason }}</span>
          </p>
        </div>
        <div class="cli-trust-item">
          <p><strong>Claude CLI</strong></p>
          <div class="row">
            <input v-model="claudePathDraft" placeholder="claude 或完整路径" />
            <button @click="handleConfirmClaudePath">解析并确认</button>
            <button @click="handlePickClaudeFile">选择文件…</button>
          </div>
          <p v-if="store.claudeTrust">
            <span :class="store.claudeTrust.trusted ? 'ok' : 'bad'">{{ store.claudeTrust.trusted ? '已信任' : '未信任' }}</span>
            — 解析路径：<code>{{ store.claudeTrust.canonical_path }}</code>
            <span v-if="store.claudeTrust.mismatch_reason"> — {{ store.claudeTrust.mismatch_reason }}</span>
          </p>
        </div>
      </div>
    </section>

    <section v-if="store.currentProject" class="block">
      <h3>2. 新建任务（自然语言需求）</h3>
      <div class="form-grid">
        <input v-model="taskTitle" placeholder="任务标题" />
        <textarea v-model="taskGoal" placeholder="目标 / 需求描述"></textarea>
        <textarea v-model="taskInScope" placeholder="本次范围内"></textarea>
        <textarea v-model="taskNotInScope" placeholder="明确不包含"></textarea>
        <textarea v-model="taskDoneCriteria" placeholder="完成标准"></textarea>
      </div>
      <div class="row">
        <label>
          开发 AI：
          <select v-model="store.devAiKind" :disabled="!store.canChangeRoles">
            <option value="codex_cli">Codex</option>
            <option value="claude_cli">Claude Code</option>
          </select>
        </label>
        <button @click="handleCreateTask">创建任务</button>
      </div>
      <ul class="project-list">
        <li v-for="task in store.tasks" :key="task.id">
          <button :class="{ active: store.currentTask?.id === task.id }" @click="store.selectTask(task)">
            {{ task.title }} — {{ stateLabels[task.state] ?? task.state }}
          </button>
        </li>
      </ul>
    </section>

    <section v-if="store.currentTask" class="block">
      <h3>3. 确认需求与测试命令</h3>
      <p><strong>当前任务状态：</strong>{{ stateLabels[store.currentTask.state] ?? store.currentTask.state }}</p>
      <p v-if="store.currentTask.document_sync_pending" class="hint">
        文档同步中/失败，将自动重试。
        <button class="link-button" @click="store.resyncDocuments">立即重试文档同步</button>
      </p>
      <p v-if="store.currentTask.state === 'draft'">
        <button @click="store.confirmRequirement">我已确认需求，继续</button>
      </p>

      <details v-if="!['draft', 'completed', 'failed', 'cancelled'].includes(store.currentTask.state)">
        <summary>查看原需求并确认沿用</summary>
        <p>仅将已有需求生成结构化清单，不改写原文，也不解除历史任务冻结。</p>
        <p><strong>目标：</strong><span class="requirement-text">{{ store.currentTask.goal || '（空）' }}</span></p>
        <p><strong>本次范围：</strong><span class="requirement-text">{{ store.currentTask.in_scope || '（空）' }}</span></p>
        <p><strong>不包含：</strong><span class="requirement-text">{{ store.currentTask.not_in_scope || '（空）' }}</span></p>
        <p><strong>完成标准：</strong><span class="requirement-text">{{ store.currentTask.done_criteria || '（空）' }}</span></p>
        <button :disabled="store.snapshotGenerating" @click="store.generateRequirementSnapshot">
          {{ store.snapshotGenerating ? '生成中…' : '确认沿用，生成需求清单' }}
        </button>
        <p v-if="store.snapshotMessage">{{ store.snapshotMessage }}</p>
      </details>

      <div v-if="store.currentTask.test_command_source">
        <p><strong>测试命令（{{ store.currentTask.test_command_source === 'auto' ? '自动检测' : '用户确认' }}）：</strong>
          {{ store.currentTask.test_command_redacted }}</p>
      </div>
      <div v-else>
        <p v-if="store.detectedTestCommand">
          检测到测试命令：<code>{{ store.detectedTestCommand }}</code>
          <button @click="handleConfirmTestCommand('auto')">确认使用</button>
        </p>
        <p v-else>未检测到支持的测试框架（package.json / Cargo.toml / pyproject.toml）。</p>
        <div class="row">
          <input v-model="testCommandDraft" placeholder="手动输入测试命令，例如 cargo test" />
          <button @click="handleConfirmTestCommand('user')">确认使用此命令</button>
        </div>
      </div>
    </section>

    <!-- Second re-review remediation (Blocker 3), backend-authoritative
         source per Work Package 3 final review remediation (Major 1): the
         explicit no-tests confirmation card. Neither choice is
         pre-selected. Only ever shown when the backend itself has
         confirmed the task is genuinely stalled on "no detectable test
         command", never guessed from the frontend alone. Also doubles as
         the recovery UI for a half-success confirmation: calling this
         again after a prior partial failure safely completes the
         reconciliation. -->
    <section v-if="store.noTestsConfirmationNeeded" class="block no-tests-card">
      <h3>⚠ 未检测到可运行的自动测试</h3>
      <p>Nova 未检测到可运行的自动测试。是否确认该项目目前没有适用测试？</p>
      <div class="row">
        <label><input type="radio" v-model="noTestsChoice" value="confirm" /> 确认没有适用测试</label>
        <label><input type="radio" v-model="noTestsChoice" value="provide-command" /> 返回填写测试命令</label>
      </div>
      <div v-if="noTestsChoice === 'confirm'" class="row">
        <input v-model="noTestsReason" placeholder="原因（可选）" :disabled="store.confirmingNoTests" />
        <button :disabled="store.confirmingNoTests" @click="handleConfirmNoTests">
          {{ store.confirmingNoTests ? '确认中…' : '确认没有适用测试' }}
        </button>
      </div>
      <div v-else-if="noTestsChoice === 'provide-command'" class="row">
        <input v-model="testCommandDraft" placeholder="手动输入测试命令，例如 cargo test" />
        <button @click="handleConfirmTestCommand('user')">确认使用此命令</button>
      </div>
    </section>

    <!-- v0.1.0 scope correction (Work Package 2): the explicit no-build
         confirmation card — the frontend counterpart to the backend's
         already-existing `confirm_no_build_applicable`/
         `no_build_confirmation_needed`. Only ever shown when the backend
         itself has confirmed the task is genuinely stalled on build
         detection, never guessed from the frontend alone. Also doubles as
         the recovery UI for a half-success confirmation: calling this
         again after a prior partial failure safely completes the
         reconciliation. -->
    <section v-if="store.noBuildConfirmationNeeded" class="block no-tests-card">
      <h3>⚠ 无法判断该项目是否需要构建</h3>
      <p>Nova 无法自动判断这个项目是否需要构建步骤。是否确认该项目当前无需构建？</p>
      <div class="row">
        <input v-model="noBuildReason" placeholder="原因（可选）" :disabled="store.confirmingNoBuild" />
        <button :disabled="store.confirmingNoBuild" @click="handleConfirmNoBuild">
          {{ store.confirmingNoBuild ? '确认中…' : '确认当前无需构建' }}
        </button>
      </div>
    </section>

    <section v-if="store.currentTask && ['confirmed', 'queued'].includes(store.currentTask.state)" class="block">
      <h3>4. CLI 就绪检查与启动</h3>
      <button @click="store.checkReadiness">检查 Codex / Claude CLI 就绪状态</button>
      <div v-if="store.readiness" class="readiness">
        <p>
          <strong>开发（{{ store.readiness.dev_kind }}）：</strong>
          <span :class="store.readiness.dev_ready ? 'ok' : 'bad'">{{ store.readiness.dev_ready ? '就绪' : '未就绪' }}</span>
          — {{ store.readiness.dev_message }}
        </p>
        <!-- Codex re-review remediation (Blocker 1 / Minor 2): the other AI
             is never a v0.1.0 requirement — shown purely as an optional,
             non-blocking diagnostic, never colored as a blocking "未就绪". -->
        <p class="hint">
          <strong>另一位 AI（{{ store.readiness.reviewer_kind }}，可选诊断，不影响启动）：</strong>
          {{ store.readiness.review_message }}
        </p>
      </div>
      <button
        :disabled="store.starting || !store.canStart"
        @click="handleStart"
      >
        {{ store.starting ? '启动中…' : '开始任务' }}
      </button>
    </section>

    <section v-if="store.currentTask && store.isRunning" class="block">
      <h3>5. 运行中</h3>
      <p>当前状态：{{ stateLabels[store.currentTask.state] ?? store.currentTask.state }}</p>
      <button @click="store.cancelTask">取消任务</button>
      <button class="link-button" @click="store.refreshCurrentTask">手动刷新状态</button>
    </section>

    <section
      v-if="store.currentTask && ['preview_ready', 'awaiting_user_acceptance', 'failed', 'needs_user_action', 'cancelled', 'completed'].includes(store.currentTask.state)"
      class="block"
    >
      <h3>6. 结果与验收</h3>
      <!-- v0.1.0 scope correction (Work Package 2): review records are an
           optional diagnostic — v0.1.0's result and completion never
           depend on them, so this section is framed around Nova's own
           verification and the user's acceptance, not a review verdict. -->
      <details class="reviews-diagnostic">
        <summary>审核记录（可选诊断信息，v0.2.0 双 AI 审核能力，不影响本次结果）</summary>
        <button class="link-button" @click="store.loadReviews">加载审核记录</button>
        <div v-for="review in store.reviews" :key="review.id" class="review-item">
          <p><strong>第 {{ review.round }} 轮 · {{ review.reviewer_ai_kind }}：</strong>{{ review.verdict }}</p>
          <p class="findings">{{ review.findings_summary }}</p>
        </div>
        <p v-if="store.reviews.length === 0" class="hint">暂无审核记录。</p>
      </details>

      <!-- WP2A: a read-only precheck of the backend's `completed` gate,
           shown ahead of the real accept/reject decision so the user is
           not surprised by a refusal only after clicking "通过验收". Never
           itself blocks the buttons below — `accept_task_result` always
           re-checks the same gate authoritatively at the moment of the
           real attempt; this is purely an early, non-authoritative
           explanation. -->
      <p
        v-if="store.currentTask.state === 'awaiting_user_acceptance' && store.completionPrecheck && !store.completionPrecheck.can_submit_acceptance"
        class="bad precheck-banner"
      >
        ⚠ 当前结果尚不满足提交验收条件：{{ store.completionPrecheck.message }}
      </p>

      <!-- WP2A: the developer AI's own structured self-assessment of the
           adopted round — audit/context display only, never a gate
           decision made on the frontend. -->
      <p v-if="store.completionPrecheckError" class="bad">{{ store.completionPrecheckError }}</p>
      <p v-if="store.selfChecksError" class="bad">{{ store.selfChecksError }}</p>
      <details v-if="latestSelfCheck" class="reviews-diagnostic">
        <summary>开发 AI 自检摘要（第 {{ latestSelfCheck.round }} 轮，仅供参考）</summary>
        <p v-if="latestSelfCheck.parse_status === 'invalid_format'" class="bad">
          自检记录格式无效，无法解析（不满足完工条件）。
        </p>
        <template v-else>
          <p>
            自评状态：
            <span :class="latestSelfCheck.self_reported_status === 'self_reported_complete' ? 'ok' : 'bad'">
              {{ selfReportedStatusLabels[latestSelfCheck.self_reported_status ?? ''] ?? latestSelfCheck.self_reported_status }}
            </span>
          </p>
          <p v-if="latestSelfCheckCheckResults.length > 0">
            已执行检查：
            <ul class="self-check-list">
              <li v-for="(item, index) in latestSelfCheckCheckResults" :key="index">{{ item }}</li>
            </ul>
          </p>
          <p v-if="latestSelfCheckUnfinishedItems.length > 0" class="bad">
            未完成项：
            <ul class="self-check-list">
              <li v-for="(item, index) in latestSelfCheckUnfinishedItems" :key="index">{{ item }}</li>
            </ul>
          </p>
          <p v-if="latestSelfCheckBlockingIssues.length > 0" class="bad">
            阻断问题：
            <ul class="self-check-list">
              <li v-for="(item, index) in latestSelfCheckBlockingIssues" :key="index">{{ item }}</li>
            </ul>
          </p>
        </template>
      </details>

      <div v-if="store.currentTask.state === 'awaiting_user_acceptance'" class="accept-block">
        <textarea v-model="acceptNote" placeholder="验收备注（可选）"></textarea>
        <div class="row">
          <button :disabled="!store.completionPrecheck?.can_submit_acceptance" @click="store.acceptResult(true, acceptNote)">通过验收</button>
          <button @click="store.acceptResult(false, acceptNote)">不通过</button>
        </div>
      </div>
      <p v-if="store.currentTask.state === 'completed'" class="ok">任务已完成，用户已验收。</p>
      <p v-if="store.currentTask.state === 'failed'" class="bad">Nova 独立测试/构建验证未通过（已尝试一次修复），需要用户决定下一步。</p>
      <p
        v-if="store.currentTask.state === 'needs_user_action' && !store.noTestsConfirmationNeeded && !store.noBuildConfirmationNeeded"
        class="bad"
      >
        需要用户处理（详见任务状态历史）。
      </p>
      <p v-if="store.currentTask.state === 'cancelled'">任务已被用户取消。</p>
    </section>

    <!-- Phase 4 (evidence and document gate): a real timeline derived
         entirely from already-persisted facts (state transitions, the
         Action Ledger, test/build evidence, user confirmations and
         acceptance) — never a fabricated progress percentage, and never an
         event for something that has not actually happened. Available for
         a task in any state, including terminal ones — viewing evidence
         never re-executes anything. -->
    <section v-if="store.currentTask" class="block">
      <h3>7. 时间线与 Nova 验证</h3>
      <div v-if="store.evidenceOverview">
        <p>
          <strong>当前阶段：</strong>{{ stateLabels[store.evidenceOverview.task_state] ?? store.evidenceOverview.task_state }}
          <span v-if="store.isRunning" class="hint"> · 运行中</span>
        </p>
        <p v-if="latestTimelineEntry" class="hint">
          最近活动：{{ latestTimelineEntry.title }} — {{ statusLabel(latestTimelineEntry.status) }}
        </p>
        <details class="timeline-detail">
          <summary>查看完整时间线（{{ store.evidenceOverview.timeline.length }} 条）</summary>
          <ul class="timeline-list">
            <li v-for="entry in store.evidenceOverview.timeline" :key="entry.id">
              <span>{{ entry.title }}</span> —
              <span :class="timelineEntryClass(entry)">{{ statusLabel(entry.status) }}</span>
              <span v-if="entry.is_historical" class="hint">（历史）</span>
              <span v-if="entry.needs_user_action" class="bad">（需要用户处理）</span>
            </li>
          </ul>
        </details>

        <h4>Nova 验证摘要</h4>
        <p>改动文件：{{ store.evidenceOverview.file_changes.length }} 个</p>
        <p v-if="latestGitEvent">
          Git：{{ latestGitEvent.branch ?? '（未知分支）' }} · {{ latestGitEvent.commit_sha ?? '（无提交）' }}
          <span v-if="latestGitEvent.diff_stat"> · {{ latestGitEvent.diff_stat }}</span>
        </p>
        <p>测试：<span :class="badStatuses.has(currentTests[currentTests.length - 1]?.status) ? 'bad' : 'ok'">{{ currentTestStatusLabel }}</span></p>
        <p>构建：<span :class="badStatuses.has(currentBuilds[currentBuilds.length - 1]?.status) ? 'bad' : 'ok'">{{ currentBuildStatusLabel }}</span></p>
        <p>
          是否满足提交用户验收条件：
          <span :class="store.evidenceOverview.acceptance_ready ? 'ok' : 'hint'">{{ store.evidenceOverview.acceptance_ready ? '可以提交验收' : '尚不满足验收条件' }}</span>
        </p>
        <details class="evidence-detail">
          <summary>技术详情（Nova 执行过的命令，已脱敏）</summary>
          <div v-for="cmd in store.evidenceOverview.commands" :key="cmd.id" class="evidence-item">
            <code>{{ cmd.command_redacted }}</code> — exit {{ cmd.exit_code ?? '(无)' }}
          </div>
          <p v-if="store.evidenceOverview.commands.length === 0" class="hint">暂无命令记录。</p>
        </details>
      </div>
      <button class="link-button" @click="store.loadEvidenceOverview">刷新时间线与证据</button>
    </section>

    <!-- Phase 4: the seven result documents, listed by their real
         generation/existence status and shown only in their redacted form
         — no arbitrary file path is ever accepted from the UI. -->
    <section v-if="store.currentTask" class="block">
      <h3>8. 任务文档</h3>
      <ul class="document-list">
        <li v-for="doc in store.taskDocuments" :key="doc.document_type">
          <span>{{ doc.display_name }}</span>
          <span v-if="!doc.exists" class="hint">（尚未生成）</span>
          <button v-else class="link-button" @click="handleViewDocument(doc.document_type)">查看</button>
        </li>
      </ul>
      <button class="link-button" @click="store.loadTaskDocuments">刷新文档列表</button>
      <div v-if="store.selectedDocument" class="document-content">
        <h4>{{ store.selectedDocument.display_name }}</h4>
        <p v-if="store.selectedDocument.error" class="bad">{{ store.selectedDocument.error }}</p>
        <pre v-else-if="store.selectedDocument.content">{{ store.selectedDocument.content }}</pre>
        <p v-else class="hint">该文档尚未生成。</p>
        <button class="link-button" @click="store.selectedDocument = null">关闭</button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.requirement-text { white-space: pre-wrap; }
.core-loop-view {
  padding: 16px;
  max-width: 720px;
}
.block {
  margin-bottom: 20px;
  padding-bottom: 16px;
  border-bottom: 1px solid #333;
}
.row {
  display: flex;
  gap: 8px;
  margin: 8px 0;
  flex-wrap: wrap;
  align-items: center;
}
.form-grid {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.project-list {
  list-style: none;
  padding: 0;
  margin: 8px 0;
}
.project-list button {
  width: 100%;
  text-align: left;
}
.project-list button.active {
  font-weight: bold;
}
.link-button {
  background: none;
  border: none;
  text-decoration: underline;
  cursor: pointer;
  padding: 4px 0;
}
.error-banner {
  color: #d9534f;
  font-weight: bold;
}
.hint {
  color: #999;
  font-size: 0.9em;
}
.readonly-path {
  width: 100%;
  background: #222;
  color: #ccc;
  border: 1px solid #444;
}
.readiness .ok,
.ok {
  color: #4caf50;
}
.readiness .bad,
.bad {
  color: #d9534f;
}
.precheck-banner {
  border: 1px solid #d9534f;
  border-radius: 4px;
  padding: 8px;
  font-weight: bold;
}
.self-check-list {
  margin: 4px 0 4px 20px;
  padding: 0;
}
.review-item {
  border: 1px solid #444;
  border-radius: 4px;
  padding: 8px;
  margin: 8px 0;
}
.findings {
  white-space: pre-wrap;
  font-size: 0.9em;
}
.accept-block textarea {
  width: 100%;
  min-height: 60px;
}
.no-tests-card {
  border: 1px solid #d9534f;
  border-radius: 6px;
  padding: 12px;
}
.cli-trust-grid {
  display: flex;
  gap: 16px;
  flex-wrap: wrap;
}
.cli-trust-item {
  flex: 1;
  min-width: 260px;
}
.timeline-list {
  list-style: none;
  padding: 0;
  margin: 8px 0;
}
.timeline-list li {
  padding: 4px 0;
  border-bottom: 1px solid #333;
}
.evidence-item {
  font-size: 0.9em;
  padding: 4px 0;
  word-break: break-all;
}
.document-list {
  list-style: none;
  padding: 0;
  margin: 8px 0;
}
.document-list li {
  display: flex;
  gap: 8px;
  align-items: center;
  padding: 4px 0;
}
.document-content {
  border: 1px solid #444;
  border-radius: 4px;
  padding: 8px;
  margin-top: 8px;
}
.document-content pre {
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 400px;
  overflow-y: auto;
}
</style>
