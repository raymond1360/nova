<script setup lang="ts">
import { computed, onUnmounted, ref } from 'vue'
import { useWorkspaceStore, type RequirementFields } from '../../stores/workspace'
import type { Language } from './text'
const props = defineProps<{ projectId: string; projectName: string; taskId?: string; language: Language }>()
const emit = defineEmits<{ close: [] }>()
const store = useWorkspaceStore()
// Capture the approved opening scope once; selection elsewhere cannot retarget this editor.
const project = props.projectId, task = props.taskId
const scopeKey = ref(''), opened = ref(false), leaving = ref(false), refreshing = ref(false)
let destination: (() => void) | undefined
const e = computed(() => scopeKey.value ? store.entries[scopeKey.value] : null)
const t = (zh: string, en: string) => props.language === 'zh' ? zh : en
const fields: [keyof RequirementFields, string, string][] = [
  ['title_raw', '标题', 'Title'], ['goal_raw', '目标', 'Goal'], ['in_scope_raw', '范围内', 'In scope'],
  ['not_in_scope_raw', '不包含项', 'Out of scope'], ['done_criteria_raw', '完成标准', 'Completion criteria'],
]
const pending = computed(() => !!(e.value?.saving || e.value?.loading || refreshing.value))
const editable = computed(() => e.value?.draft?.status === 'editable' && !e.value.confirming && !e.value.confirmRetry)
const canRefresh = computed(() => !!e.value?.draft && !pending.value && !e.value.dirty && !e.value.composing && !e.value.saveRetry && !e.value.confirmRetry && !e.value.messagePending && !e.value.confirming && !e.value.conflict)
async function open(origin: string | null) {
  if (opened.value || !project) return
  opened.value = true; scopeKey.value = `${project}:${origin ?? 'initial'}`
  await store.open(project, origin)
}
async function retryOpen() {
  const entry = e.value
  if (!entry || entry.loading || entry.draft) return
  delete store.entries[scopeKey.value]
  await store.open(project, entry.origin)
}
function edit(field: keyof RequirementFields, event: Event) {
  if (store.activeKey !== scopeKey.value) return
  store.edit(field, (event.target as HTMLInputElement).value)
}
function composition(active: boolean) { if (e.value) e.value.composing = active }
async function refresh() {
  if (!canRefresh.value) return
  refreshing.value = true
  try { await store.refresh(scopeKey.value); if (e.value) e.value.error = '' }
  catch { if (e.value) e.value.error = 'REQUEST_FAILED' }
  finally { refreshing.value = false }
}
function finishLeave() {
  if (pending.value) return
  if (e.value) e.value.composing = false
  const go = destination; destination = undefined; leaving.value = false
  store.leave(); go?.()
}
function requestLeave(go: () => void = () => emit('close')) {
  if (pending.value) return
  destination = go
  if (e.value?.dirty || e.value?.composing || e.value?.saveRetry) leaving.value = true
  else finishLeave()
}
defineExpose({ requestLeave })
onUnmounted(() => { if (e.value) e.value.composing = false; if (store.activeKey === scopeKey.value) store.leave() })
</script>

<template>
  <section class="rd-editor" data-draft-editor :aria-label="t('正式需求草稿编辑', 'Persistent requirement draft editor')">
    <header><h1>{{ t('正式持久草稿；未确认、未创建任务', 'Persistent draft; not confirmed, no task created here') }}</h1>
      <p>{{ t('项目', 'Project') }}: {{ projectName }} · {{ project }}</p>
      <button type="button" data-draft-back :disabled="pending" @click="requestLeave()">{{ t('返回只读预览', 'Back to read-only preview') }}</button>
    </header>
    <p class="rd-warning" data-reload-warning>{{ t('未保存输入只保留在本窗口内存；刷新或关闭可能丢失。保存中关闭可能使结果未知，请勿另建请求重复保存。只点击保存按钮才保存。', 'Unsaved input stays in window memory only and may be lost on reload or close. Closing during a save may leave its result unknown; do not create a duplicate save request. Only Save writes your edits.') }}</p>
    <div v-if="!opened" data-open-notice>
      <p>{{ t('明确打开后会访问产品库，必要时创建会话和空草稿；不会确认需求或创建任务。不会导入本窗口草稿。', 'Opening accesses the product database and may create a conversation and empty draft. It does not confirm requirements, create a task or import your window draft.') }}</p>
      <p>{{ t('来源任务', 'Source task') }}: {{ task ?? t('无（项目草稿）', 'None (project draft)') }}</p>
      <button type="button" data-open-project @click="open(null)">{{ t('确认打开项目草稿', 'Confirm: open project draft') }}</button>
      <button v-if="task" type="button" data-open-next @click="open(task)">{{ t('确认打开此任务的下一轮草稿', 'Confirm: open next-round draft for this task') }}</button>
    </div>
    <p v-if="e?.loading" role="status">{{ t('正在打开…', 'Opening…') }}</p>
    <p v-if="e?.error" role="alert">{{ t('操作未完成，输入已保留；未确认保存结果。错误码：', 'Request incomplete; input preserved, save result unconfirmed. Code: ') }}{{ e.error }}</p>
    <button v-if="e?.error && !e.draft && !e.loading" type="button" data-retry-open @click="retryOpen">{{ t('手动重试打开', 'Retry opening manually') }}</button>
    <template v-if="e?.draft">
      <p>{{ t('草稿来源任务', 'Draft source task') }}: {{ e.origin ?? t('无（项目草稿）', 'None (project draft)') }}</p>
      <p>{{ t('草稿状态', 'Draft status') }}: {{ e.draft.status }} · {{ t('上次读取的库内版本', 'Last read database revision') }} {{ e.draft.revision }}</p>
      <p v-if="e.draft.status === 'bound'">{{ t('此草稿已绑定已有任务，只读；本窗口未创建任务。', 'This draft is already bound to an existing task and is read only; this window did not create it.') }} {{ e.binding?.task_id }}</p>
      <label v-for="[field, zh, en] in fields" :key="field">{{ t(zh, en) }}
        <textarea :data-formal-field="field" :value="e.fields[field] ?? ''" :readonly="!editable" @input="edit(field, $event)" @compositionstart="composition(true)" @compositionend="composition(false); edit(field, $event)"></textarea>
      </label>
      <label>{{ t('执行AI（可不选；本包不执行）', 'Execution AI (optional; execution unavailable)') }}
        <select data-formal-ai :value="e.fields.selected_ai ?? ''" :disabled="!editable" @change="edit('selected_ai', $event)"><option value="">{{ t('未选择', 'Not selected') }}</option><option value="codex_cli">Codex CLI</option><option value="claude_cli">Claude CLI</option></select>
      </label>
      <p role="status" data-save-status>{{ e.saving ? t('保存中…', 'Saving…') : e.error || e.saveRetry ? t('保存未确认，请手动处理', 'Save unconfirmed; manual action required') : e.dirty ? t('未保存', 'Unsaved') : t('已保存版本 ', 'Saved revision ') + e.draft.revision }}</p>
      <button type="button" data-save :disabled="!editable || pending || e.composing || !!e.conflict || (!e.dirty && !e.saveRetry)" @click="store.save(scopeKey)">{{ e.saveRetry ? t('手动重试原保存请求', 'Retry original save request manually') : t('保存草稿', 'Save draft') }}</button>
      <button type="button" data-refresh-draft :disabled="!canRefresh" @click="refresh">{{ t('读取服务器版本', 'Read server revision') }}</button>
      <section v-if="e.conflict" data-conflict>
        <h2>{{ t('版本冲突：本地输入保留在上方', 'Revision conflict: local input preserved above') }}</h2>
        <p>{{ t('服务器版本', 'Server revision') }} {{ e.conflict.revision }} · {{ e.conflict.status }}</p><pre>{{ e.conflict.fields }}</pre>
        <button type="button" data-keep-local :disabled="e.conflict.status !== 'editable' || e.composing || pending" @click="store.keepLocalAfterComparison()">{{ t('已比较，保留本地输入（仍须点击保存）', 'Compared: keep local input (still requires Save)') }}</button>
      </section>
    </template>
    <section v-if="leaving" role="alertdialog" data-leave-prompt>
      <p>{{ t('输入尚未保存或保存结果未知；离开不会自动保存。', 'Input is unsaved or its save result is unknown. Leaving will not save automatically.') }}</p>
      <button type="button" data-leave-keep :disabled="pending" @click="finishLeave">{{ t('保留输入并离开', 'Keep input and leave') }}</button>
      <button type="button" data-leave-stay @click="leaving = false; destination = undefined">{{ t('继续编辑', 'Continue editing') }}</button>
    </section>
  </section>
</template>
<style scoped>
.rd-editor { flex: 1; overflow: auto; padding: 24px; max-width: 100%; min-height: 0; }
.rd-warning { position: sticky; top: 0; z-index: 2; background: var(--ro-bg); border: 1px solid var(--ro-border); padding: 10px; }
h1 { font-size: 22px; } label { display: block; margin: 14px 0; }
textarea { display: block; box-sizing: border-box; width: 100%; min-height: 75px; resize: vertical; }
button, select, textarea { font: inherit; color: inherit; border: 1px solid var(--ro-border); border-radius: 7px; padding: 9px; background: var(--ro-bg); }
button { margin: 4px; cursor: pointer; white-space: normal; } button:disabled { opacity: .5; cursor: default; }
pre { white-space: pre-wrap; overflow-wrap: anywhere; } [role=alert], [data-conflict], [data-leave-prompt] { padding: 12px; border: 1px solid #a56a36; }
[data-leave-prompt] { position: fixed; left: 24px; right: 24px; bottom: 24px; z-index: 20; background: var(--ro-bg); box-shadow: 0 6px 30px #0003; max-height: 50vh; overflow: auto; }
</style>
