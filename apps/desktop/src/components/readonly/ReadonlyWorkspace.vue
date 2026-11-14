<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useReadonlyWorkspace } from '../../stores/readonlyWorkspace'
import ReadonlyEvidenceDrawer from './ReadonlyEvidenceDrawer.vue'
import { copy, tokens, stage, type Language } from './text'
const emit = defineEmits<{ close: [] }>()
const store = useReadonlyWorkspace()
const language = ref<Language>('zh'), theme = ref<'green' | 'paper'>('green')
const drawer = ref(false), sidebar = ref(true), notificationError = ref(false)
const t = computed(() => copy[language.value])
const selectedTask = computed(() => store.tasks.find(task => task.id === store.taskId))
let disposed = false, timer: ReturnType<typeof setInterval> | undefined, unlisten: UnlistenFn | undefined
function date(value: string | null) {
  if (!value) return '—'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat(language.value === 'zh' ? 'zh-CN' : 'en', { dateStyle: 'short', timeStyle: 'medium' }).format(date)
}
function edit(event: Event) {
  const input = event.target as HTMLTextAreaElement
  store.editDraft(input.value)
  input.style.height = 'auto'; input.style.height = Math.min(input.scrollHeight, 150) + 'px'
}
onMounted(() => {
  void store.activate()
  timer = setInterval(() => { void store.refreshOverview() }, 3000)
  void listen<{ task_id: string }>('core-loop://done', event => {
    if (!disposed && event.payload.task_id === store.taskId) void store.refreshOverview()
  }).then(stop => { if (disposed) stop(); else unlisten = stop }).catch(() => { if (!disposed) notificationError.value = true })
})
onUnmounted(() => { disposed = true; if (timer) clearInterval(timer); unlisten?.(); store.dispose() })
</script>

<template>
  <div class="ro" :style="tokens[theme]" :lang="language === 'zh' ? 'zh-CN' : 'en'" @keydown.esc="drawer = false">
    <header class="ro-header">
      <div class="ro-brand"><span class="ro-mark">N</span><div><strong>Nova</strong><small>{{ t.preview }}</small></div></div>
      <span class="ro-badge">{{ t.readonly }}</span>
      <div class="ro-tools"><button type="button" data-language @click="language = language === 'zh' ? 'en' : 'zh'">{{ t.language }}</button><button type="button" @click="theme = theme === 'green' ? 'paper' : 'green'">{{ theme === 'green' ? t.theme : t.green }}</button><button type="button" data-return @click="emit('close')">{{ t.back }}</button></div>
    </header>
    <div class="ro-body" :class="{ 'drawer-open': drawer }">
      <aside v-if="sidebar" class="ro-projects" :aria-label="t.projects">
        <h2>{{ t.projects }}</h2><p v-if="store.projectsLoading" role="status">{{ t.loading }}</p>
        <p v-if="store.projectsError" role="alert">{{ t.readError }} <button @click="store.loadProjects()">{{ t.refresh }}</button></p>
        <p v-if="!store.projectsLoading && !store.projects.length && !store.projectsError">{{ t.emptyProjects }}</p>
        <button v-for="project in store.projects" :key="project.id" type="button" class="ro-choice" :class="{ selected: store.projectId === project.id }" :data-project="project.id" :aria-pressed="store.projectId === project.id" @click="store.selectProject(project.id)"><strong>{{ project.name }}</strong><small>{{ project.local_path }}</small></button>
        <div class="ro-task-heading"><h2>{{ t.tasks }}</h2></div>
        <p v-if="!store.projectId">{{ t.chooseProject }}</p><p v-if="store.tasksLoading" role="status">{{ t.loading }}</p>
        <p v-if="store.tasksError" role="alert">{{ t.readError }} <button @click="store.selectProject(store.projectId)">{{ t.refresh }}</button></p>
        <p v-if="store.projectId && !store.tasksLoading && !store.tasksError && !store.tasks.length">{{ t.emptyTasks }}</p>
        <button v-for="task in store.tasks" :key="task.id" type="button" class="ro-choice" :class="{ selected: store.taskId === task.id }" :data-task="task.id" :aria-pressed="store.taskId === task.id" @click="store.selectTask(task.id)"><strong>{{ task.title }}</strong><small>{{ stage(task.state, language) }}</small></button>
      </aside>
      <main class="ro-main">
        <div class="ro-context"><button type="button" :aria-expanded="sidebar" @click="sidebar = !sidebar">{{ t.showSidebar }}</button><span>{{ selectedTask?.title ?? t.title }}</span><button type="button" data-evidence-toggle :aria-expanded="drawer" @click="drawer = !drawer">{{ drawer ? t.close : t.evidence }}</button></div>
        <div class="ro-feed">
          <div class="ro-kicker">NOVA · {{ t.readonly }}</div><h1>{{ selectedTask?.title ?? t.title }}</h1><p class="ro-intro">{{ t.intro }}</p>
          <div v-if="!store.taskId" class="ro-empty"><span aria-hidden="true">◌</span><h2>{{ store.projectId ? t.chooseTask : t.chooseProject }}</h2></div>
          <template v-else>
            <section class="ro-record" aria-live="polite">
              <small>{{ t.recorded }}</small><h2>{{ store.overview ? stage(store.overview.task_state, language) : t.unknown }}</h2><p>{{ t.liveUnknown }}</p>
              <div v-if="store.overview" class="ro-meta"><span>{{ t.generation }} {{ store.overview.result_generation }}</span><span>{{ t.round }} {{ store.overview.pending_round }}</span></div>
              <p class="ro-read-time">{{ t.readAt }}: {{ date(store.lastSuccessfulReadAt) }}</p>
              <p v-if="store.overviewLoading" role="status">{{ t.loading }}</p>
              <p v-if="store.overviewError" role="alert" class="ro-warning">{{ t.readError }}<br><span v-if="store.overview">{{ t.stale }}</span></p>
              <button type="button" :disabled="store.overviewLoading" @click="store.refreshOverview()">{{ t.refresh }}</button>
            </section>
            <section class="ro-timeline"><h2>{{ t.timeline }}</h2><p v-if="!store.overview?.timeline.length">{{ t.emptyTimeline }}</p>
              <ol><li v-for="item in store.overview?.timeline ?? []" :key="item.id" :data-timeline="item.id">
                <div class="ro-event-dot" aria-hidden="true"></div><div class="ro-event"><h3>{{ item.title }}</h3><p>{{ stage(item.status, language) }} <span v-if="item.is_historical" class="ro-tag">{{ t.historical }}</span> <span v-if="item.needs_user_action">{{ t.needsAction }}</span></p>
                <small>{{ t.eventAt }}: <time :datetime="item.started_at ?? undefined">{{ date(item.started_at ?? item.finished_at) }}</time></small>
                <small v-if="item.result_generation !== null">{{ t.generation }} {{ item.result_generation }} · {{ t.round }} {{ item.round ?? '—' }}</small></div>
              </li></ol>
            </section>
          </template>
          <p v-if="notificationError" class="ro-warning" role="status">{{ language === 'zh' ? '事件通知不可用，仍每3秒只读刷新。' : 'Event notifications unavailable; read-only polling continues every 3 seconds.' }}</p>
        </div>
        <footer class="ro-composer"><label for="ro-draft">{{ t.draft }}</label><textarea id="ro-draft" :value="store.draft" :disabled="!store.projectId" :placeholder="t.placeholder" rows="2" @input="edit"></textarea><p v-if="store.cacheFailed" class="ro-warning" role="alert">{{ t.cacheFailed }}</p><div class="ro-composer-bottom"><small>{{ t.draftHint }}</small><button type="button" :disabled="true">{{ t.disabled }}</button></div></footer>
      </main>
      <ReadonlyEvidenceDrawer v-if="drawer" :overview="store.overview" :language="language" @close="drawer = false" />
    </div>
  </div>
</template>

<style scoped>
.ro { height: 100dvh; width: 100%; display: flex; flex-direction: column; overflow: hidden; background: var(--ro-bg); color: #202b24; font-family: 'Segoe UI', 'Microsoft YaHei', sans-serif; font-size: 14px; }
.ro * { box-sizing: border-box; } .ro-header { min-height: 76px; padding: 14px 24px; display: flex; align-items: center; gap: 20px; background: #fff; border-bottom: 1px solid var(--ro-border); flex-wrap: wrap; }
.ro-brand { display: flex; gap: 10px; align-items: center; } .ro-brand strong { font-size: 21px; letter-spacing: -.6px; } .ro-brand small { display: block; color: #526057; font-size: 11px; margin-top: 2px; }
.ro-mark { width: 36px; height: 36px; border-radius: 10px; background: var(--ro-accent); color: #fff; display: grid; place-items: center; font-size: 20px; font-weight: 700; }
.ro-badge { font-size: 11px; color: var(--ro-accent); background: var(--ro-soft); padding: 5px 10px; border-radius: 20px; } .ro-tools { margin-left: auto; display: flex; flex-wrap: wrap; gap: 8px; }
.ro button { min-height: 32px; padding: 6px 10px; border-radius: 6px; border: 1px solid var(--ro-border); background: #fff; color: #354a3b; cursor: pointer; font: inherit; font-size: 12px; white-space: normal; }
.ro button:hover:not(:disabled) { background: var(--ro-soft); } .ro button:disabled { cursor: default; color: #626b65; background: #edf0ed; }
.ro button:focus-visible, .ro textarea:focus-visible { outline: 2px solid var(--ro-accent); outline-offset: 2px; }
.ro-body { position: relative; display: flex; flex: 1; min-height: 0; } .ro-projects { flex: 0 0 230px; width: 230px; padding: 25px 14px; background: var(--ro-side); border-right: 1px solid var(--ro-border); overflow-y: auto; }
.ro-projects h2 { font-size: 11px; font-weight: 700; color: #526057; letter-spacing: .6px; margin: 0 10px 12px; }.ro-projects p { font-size: 12px; padding: 0 10px; line-height: 1.7; }
.ro .ro-choice { display: block; width: 100%; text-align: left; padding: 11px 12px; margin-bottom: 5px; background: transparent; border: 1px solid transparent; } .ro-choice strong { display: block; font-weight: 500; overflow-wrap: anywhere; }.ro-choice small { display: block; color: #526057; font-size: 11px; margin-top: 5px; overflow-wrap: anywhere; }.ro .ro-choice.selected { background: #fff; border-color: var(--ro-border); color: var(--ro-accent); }
.ro-task-heading { margin-top: 32px; }.ro-main { display: flex; flex: 1; flex-direction: column; min-width: 0; min-height: 0; }.ro-context { min-height: 53px; padding: 10px 24px; display: flex; align-items: center; gap: 12px; border-bottom: 1px solid var(--ro-border); }.ro-context span { flex: 1; font-size: 12px; overflow-wrap: anywhere; }.ro-feed { padding: 30px clamp(20px, 4vw, 58px); overflow-y: auto; flex: 1; min-height: 0; }.ro-kicker { color: var(--ro-accent); font-size: 10px; letter-spacing: 1.3px; font-weight: 700; }.ro h1 { font-size: 27px; font-weight: 600; letter-spacing: -.8px; margin: 12px 0; overflow-wrap: anywhere; }.ro-intro { font-size: 13px; color: #526057; line-height: 1.8; margin-bottom: 25px; }.ro-empty { padding: 48px 0; text-align: center; color: #526057; }.ro-empty>span { font-size: 42px; color: var(--ro-accent); }.ro-empty h2 { font-size: 16px; font-weight: 500; }
.ro-record { background: #fff; border: 1px solid var(--ro-border); border-radius: 10px; padding: 20px 24px; }.ro-record>small { color: var(--ro-accent); font-size: 11px; }.ro-record h2 { margin: 8px 0; font-size: 19px; font-weight: 600; }.ro-record p { color: #526057; font-size: 12px; }.ro-meta { display: flex; gap: 14px; font-size: 11px; }.ro-record .ro-read-time { font-size: 11px; }.ro-timeline { margin-top: 30px; }.ro-timeline>h2 { font-size: 13px; margin-bottom: 21px; }.ro-timeline ol { padding: 0; list-style: none; }.ro-timeline li { display: flex; gap: 15px; padding-bottom: 22px; position: relative; }.ro-event-dot { flex: 0 0 8px; height: 8px; background: var(--ro-accent); border-radius: 50%; margin-top: 6px; }.ro-event { min-width: 0; }.ro-event h3 { font-size: 14px; font-weight: 500; margin: 0 0 5px; overflow-wrap: anywhere; }.ro-event p { font-size: 12px; margin: 0 0 5px; color: #526057; }.ro-event small { display: block; color: #526057; font-size: 11px; line-height: 1.7; }.ro-tag { padding: 2px 6px; background: var(--ro-side); border-radius: 4px; margin-left: 6px; }
.ro-composer { padding: 16px 24px; border-top: 1px solid var(--ro-border); background: #fff; }.ro-composer label { display: block; font-size: 11px; color: var(--ro-accent); font-weight: 600; margin-bottom: 8px; }.ro textarea { width: 100%; min-height: 60px; max-height: 150px; resize: vertical; border: 1px solid var(--ro-border); background: var(--ro-bg); color: #202b24; border-radius: 7px; padding: 10px 12px; font: inherit; line-height: 1.6; }.ro-composer-bottom { display: flex; justify-content: space-between; align-items: center; gap: 15px; margin-top: 8px; }.ro-composer-bottom small { color: #526057; font-size: 10px; line-height: 1.6; }.ro .ro-warning { color: #864a10; line-height: 1.7; font-size: 12px; }
@media (max-width: 1100px) { .drawer-open .ro-projects { display: none; } }
@media (max-width: 760px) { .ro-header { padding: 10px 14px; gap: 8px; }.ro-badge { display: none; }.ro-projects { flex-basis: 190px; width: 190px; padding: 16px 8px; }.ro-context { padding: 8px 12px; gap: 6px; }.ro-feed { padding: 22px 18px; }.ro-composer { padding: 12px; }.ro-composer-bottom { align-items: flex-start; flex-direction: column; }.ro h1 { font-size: 23px; } }
@media (prefers-reduced-motion: reduce) { .ro * { scroll-behavior: auto; transition: none; } }
</style>
