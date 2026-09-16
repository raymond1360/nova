<script setup lang="ts">
// Phase 3 remediation (Major 1): the minimal Open-Local-Project Dual-AI
// Core Loop UI. Every action here calls a real Tauri command through
// `useCoreLoopStore` — nothing here is a static demo. Deliberately does
// NOT implement: the full Phase 4 timeline, the evidence center, GitHub
// import, Create Product, auto-deployment, or push/PR — those are out of
// this round's scope.
import { onUnmounted, ref } from 'vue'
import { useCoreLoopStore } from '../stores/coreLoop'
import type { Project } from '../types'

const store = useCoreLoopStore()

const newProjectPath = ref('')
const newProjectName = ref('')
const taskTitle = ref('')
const taskGoal = ref('')
const taskInScope = ref('')
const taskNotInScope = ref('')
const taskDoneCriteria = ref('')
const testCommandDraft = ref('')
const acceptNote = ref('')

let pollHandle: ReturnType<typeof setInterval> | null = null

function startPolling() {
  stopPolling()
  pollHandle = setInterval(() => {
    if (store.isRunning) {
      store.refreshCurrentTask().catch(() => {})
    }
  }, 3000)
}
function stopPolling() {
  if (pollHandle) {
    clearInterval(pollHandle)
    pollHandle = null
  }
}
startPolling()
onUnmounted(stopPolling)

async function handleOpenProject() {
  if (!newProjectPath.value.trim()) return
  await store.openProject(newProjectPath.value.trim(), newProjectName.value.trim() || newProjectPath.value.trim())
  newProjectPath.value = ''
  newProjectName.value = ''
}

function handleSelectProject(project: Project) {
  store.selectProject(project)
  store.loadTasks()
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

async function handleStart() {
  await store.checkReadiness()
  if (!store.readiness?.dev_ready || !store.readiness?.review_ready) return
  await store.startTask()
  startPolling()
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
</script>

<template>
  <div class="core-loop-view">
    <h2>项目与任务（Phase 3 最小闭环）</h2>
    <p v-if="store.lastError" class="error-banner">{{ store.lastError }}</p>

    <section class="block">
      <h3>1. 选择或打开本地项目</h3>
      <div class="row">
        <input v-model="newProjectPath" placeholder="项目本地路径，例如 D:\my-project" />
        <input v-model="newProjectName" placeholder="项目名称（可选）" />
        <button @click="handleOpenProject">打开/注册项目</button>
      </div>
      <button class="link-button" @click="store.loadProjects">刷新项目列表</button>
      <ul class="project-list">
        <li v-for="project in store.projects" :key="project.id">
          <button :class="{ active: store.currentProject?.id === project.id }" @click="handleSelectProject(project)">
            {{ project.name }} — {{ project.local_path }}
          </button>
        </li>
      </ul>
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
            <option value="codex_cli">Codex（Codex 开发 / Claude 审核）</option>
            <option value="claude_cli">Claude（Claude 开发 / Codex 审核）</option>
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
      <p v-if="store.currentTask.state === 'draft'">
        <button @click="store.confirmRequirement">我已确认需求，继续</button>
      </p>

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

    <section v-if="store.currentTask && store.currentTask.state === 'confirmed'" class="block">
      <h3>4. CLI 就绪检查与启动</h3>
      <button @click="store.checkReadiness">检查 Codex / Claude CLI 就绪状态</button>
      <div v-if="store.readiness" class="readiness">
        <p>
          <strong>开发（{{ store.readiness.dev_kind }}）：</strong>
          <span :class="store.readiness.dev_ready ? 'ok' : 'bad'">{{ store.readiness.dev_ready ? '就绪' : '未就绪' }}</span>
          — {{ store.readiness.dev_message }}
        </p>
        <p>
          <strong>审核（{{ store.readiness.reviewer_kind }}）：</strong>
          <span :class="store.readiness.review_ready ? 'ok' : 'bad'">{{ store.readiness.review_ready ? '就绪' : '未就绪' }}</span>
          — {{ store.readiness.review_message }}
        </p>
      </div>
      <button
        :disabled="store.starting || !store.readiness?.dev_ready || !store.readiness?.review_ready"
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
      <h3>6. 审核结论与结果</h3>
      <button class="link-button" @click="store.loadReviews">加载审核记录</button>
      <div v-for="review in store.reviews" :key="review.id" class="review-item">
        <p><strong>第 {{ review.round }} 轮 · {{ review.reviewer_ai_kind }}：</strong>{{ review.verdict }}</p>
        <p class="findings">{{ review.findings_summary }}</p>
      </div>

      <div v-if="store.currentTask.state === 'awaiting_user_acceptance'" class="accept-block">
        <textarea v-model="acceptNote" placeholder="验收备注（可选）"></textarea>
        <div class="row">
          <button @click="store.acceptResult(true, acceptNote)">通过验收</button>
          <button @click="store.acceptResult(false, acceptNote)">不通过</button>
        </div>
      </div>
      <p v-if="store.currentTask.state === 'completed'" class="ok">任务已完成，用户已验收。</p>
      <p v-if="store.currentTask.state === 'failed'" class="bad">二次审核仍未通过，或独立测试未通过，需要用户决定下一步。</p>
      <p v-if="store.currentTask.state === 'needs_user_action'" class="bad">需要用户处理（详见任务状态历史）。</p>
      <p v-if="store.currentTask.state === 'cancelled'">任务已被用户取消。</p>
    </section>
  </div>
</template>

<style scoped>
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
.readiness .ok {
  color: #4caf50;
}
.readiness .bad {
  color: #d9534f;
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
</style>
