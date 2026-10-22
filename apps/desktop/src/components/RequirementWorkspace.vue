<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { useWorkspaceStore, type RequirementFields, type Binding } from '../stores/workspace'
const props = defineProps<{ projectId: string; taskId?: string }>()
const emit = defineEmits<{ selectTask: [binding: Binding] }>()
const store = useWorkspaceStore()
const english = ref(false)
const e = computed(() => store.current)
const text = (zh: string, en: string) => english.value ? en : zh
const fields: [keyof RequirementFields, string, string][] = [
  ['title_raw', '任务标题', 'Task title'], ['goal_raw', '目标', 'Goal'], ['in_scope_raw', '范围内', 'In scope'],
  ['not_in_scope_raw', '不包含项', 'Out of scope'], ['done_criteria_raw', '完成标准', 'Completion criteria'],
]
const errorText = computed(() => {
  const code = e.value?.error
  const known: Record<string, [string, string]> = {
    REVISION_CONFLICT: ['另一窗口已保存新版本。你的输入已保留，请比较后保存。', 'A newer revision exists. Your input is preserved; compare before saving.'],
    EXECUTION_IN_PROGRESS: ['项目有任务正在执行或准备启动。请保留下一轮草稿，停止执行后再确认。', 'A task is active or reserved. Keep this next-round draft and confirm after execution stops.'],
    ACTIVITY_UNKNOWN: ['无法确认活动状态，暂不能确认。草稿已保留。', 'Activity is unknown; confirmation is blocked. Your draft is preserved.'],
    INCOMPLETE_SCOPE: ['请填写标题、四项需求并选择执行 AI。', 'Complete the title and four fields and select an AI.'],
    DRAFT_BOUND: ['此草稿已绑定任务，不能继续编辑。请打开绑定任务或创建下一轮草稿。', 'This draft is bound. Open its task or create a next-round draft.'],
  }
  return code ? (known[code] ? text(...known[code]) : text(`操作未完成（${code}），输入已保留。请手动重试；不会自动开始任务。`, `Request failed (${code}). Input is preserved. Retry manually; execution will not start automatically.`)) : ''
})
const statusText = computed(() => {
  if (e.value?.saving) return text('正在保存…', 'Saving…')
  if (e.value?.confirming) return text('正在确认…', 'Confirming…')
  if (e.value?.binding) return text('需求已绑定任务，未自动启动。', 'Bound to a task; not started automatically.')
  return e.value?.dirty ? text('有未保存修改', 'Unsaved changes') : text('已保存', 'Saved')
})
const complete = computed(() => !!e.value?.fields.selected_ai && fields.every(([f]) => !!e.value?.fields[f]?.trim()))
const editable = computed(() => e.value?.draft?.status === 'editable' && !e.value.confirming && !e.value.confirmRetry)
const timers = new Map<string, ReturnType<typeof setTimeout>>()
let disposed = false
function clearTimer(key: string) { clearTimeout(timers.get(key)); timers.delete(key) }
function clearTimers() { for (const key of timers.keys()) clearTimer(key) }
function schedule(key: string) {
  if (disposed) return
  clearTimer(key)
  timers.set(key, setTimeout(async () => {
    timers.delete(key)
    await store.save(key)
    const entry = store.entries[key]
    if (entry?.dirty && !entry.error && !entry.composing && !entry.conflict && !entry.confirming) schedule(key)
  }, 500))
}
function edit(field: keyof RequirementFields, event: Event) {
  store.edit(field, (event.target as HTMLInputElement).value)
  if (!e.value?.composing) schedule(store.activeKey)
}
function compositionStart() { if (e.value) e.value.composing = true; clearTimer(store.activeKey) }
function compositionEnd(field: keyof RequirementFields, event: Event) {
  if (e.value) e.value.composing = false
  edit(field, event)
}
async function choose(origin: string | null) {
  clearTimer(store.activeKey)
  if (e.value?.composing) e.value.composing = false
  else await store.save()
  await store.open(props.projectId, origin)
}
async function retryOpen() {
  const current = e.value
  if (current && !current.draft && !current.loading) delete store.entries[store.activeKey]
  await store.open(props.projectId, current?.origin ?? null)
}
watch(() => props.projectId, project => { clearTimer(store.activeKey); if (e.value?.composing) e.value.composing = false; else void store.save(); void store.open(project) }, { immediate: true })
onUnmounted(() => { disposed = true; clearTimers(); if (e.value?.composing) e.value.composing = false; else void store.save(); store.leave() })
</script>

<template>
  <section class="requirement-workspace block" aria-label="Requirement workspace">
    <h3>{{ text('需求草稿与版本确认（非 AI）', 'Requirement draft and revision confirmation (no AI)') }}</h3>
    <label><input type="checkbox" v-model="english" /> English</label>
    <p>{{ text('AI 整理尚未开放。请手动填写四项需求；保存和确认不会调用 AI。', 'AI planning is not available. Fill in the four fields manually; saving and confirmation do not call AI.') }}</p>
    <button :disabled="true">{{ text('AI 整理（尚未开放）', 'AI planning (unavailable)') }}</button>
    <button @click="choose(null)">{{ text('当前项目草稿', 'Project draft') }}</button>
    <button v-if="taskId" @click="choose(taskId)">{{ text('为选中任务保存下一轮草稿', 'Next-round draft for selected task') }}</button>
    <p v-if="e?.origin">{{ text('下一轮草稿，不影响原任务：', 'Next-round draft; original task is unchanged: ') }}{{ e.origin }}</p>
    <p v-if="e?.loading" role="status">{{ text('正在读取草稿…', 'Loading draft…') }}</p>
    <p v-if="errorText" role="alert">{{ errorText }}</p>
    <button v-if="e?.error && !e.draft" @click="retryOpen">{{ text('重新读取', 'Reload') }}</button>
    <template v-if="e?.draft">
      <p>{{ text('已保存版本', 'Saved revision') }} {{ e.draft.revision }} · {{ e.draft.status }}</p>
      <label v-for="[field, zh, en] in fields" :key="field" class="draft-field">
        {{ text(zh, en) }}
        <textarea :data-field="field" :value="e.fields[field] ?? ''" :readonly="!editable"
          @input="edit(field, $event)" @compositionstart="compositionStart" @compositionend="compositionEnd(field, $event)"></textarea>
      </label>
      <label>{{ text('执行 AI', 'Execution AI') }}
        <select :value="e.fields.selected_ai ?? ''" :disabled="!editable" @change="edit('selected_ai', $event)">
          <option value="">{{ text('请选择', 'Select') }}</option><option value="codex_cli">Codex CLI</option><option value="claude_cli">Claude CLI</option>
        </select>
      </label>
      <p role="status">{{ statusText }}</p>
      <p v-if="!complete">{{ text('确认前请填写标题、四项需求并选择执行 AI。', 'Complete the title and four fields and select an execution AI before confirming.') }}</p>
      <button :disabled="!editable || e.composing || e.saving || !!e.conflict" @click="store.save()">{{ text('保存草稿', 'Save draft') }}</button>
      <button :disabled="!complete || e.draft.status !== 'editable' || e.dirty || e.composing || e.saving || e.confirming || !!e.conflict || !!e.saveRetry"
        @click="store.confirm()">{{ e.confirming ? text('正在确认…', 'Confirming…') : text(`确认已保存版本 ${e.draft.revision} 并创建任务（不会自动开始）`, `Confirm saved revision ${e.draft.revision} and create task (does not start execution)`) }}</button>
      <div v-if="e.conflict" class="draft-conflict">
        <p>{{ text('远端版本（只读）；本地输入仍在上方：', 'Server revision (read-only); your local input remains above: ') }}{{ e.conflict.revision }}</p>
        <pre>{{ e.conflict.fields }}</pre>
        <button :disabled="e.conflict.status !== 'editable'" @click="store.keepLocalAfterComparison()">{{ text('已比较，保留本地输入并基于新版本保存', 'Compared: keep local input and save against the newer revision') }}</button>
      </div>
      <p v-if="e.binding">Task: {{ e.binding.task_id }}
        <button @click="emit('selectTask', e.binding!)">{{ text('查看已绑定任务', 'Open bound task') }}</button>
      </p>
      <details><summary>{{ text('原始需求消息（不会自动整理或执行）', 'Original requirement messages (no automatic planning or execution)') }}</summary>
        <p v-for="m in e.messages.filter(m => m.kind === 'user')" :key="m.id" class="raw-message">{{ m.body_raw }}</p>
        <textarea v-model="e.messageInput" :readonly="!editable" :aria-label="text('原始需求消息', 'Original requirement message')"
          @compositionstart="compositionStart" @compositionend="e.composing = false"></textarea>
        <button :disabled="!editable || e.composing || e.messagePending || e.saving || e.dirty" @click="store.append()">{{ text('保存原始消息', 'Save original message') }}</button>
      </details>
    </template>
  </section>
</template>

<style scoped>
.draft-field { display: block; margin: 10px 0; }
.draft-field textarea { display: block; width: 100%; min-height: 65px; box-sizing: border-box; }
.raw-message, pre { white-space: pre-wrap; overflow-wrap: anywhere; }
button { margin: 4px; max-width: 100%; white-space: normal; }
.draft-conflict { border: 1px solid #d97706; padding: 8px; }
</style>
