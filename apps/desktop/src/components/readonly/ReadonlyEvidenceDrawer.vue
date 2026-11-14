<script setup lang="ts">
import { computed } from 'vue'
import type { TaskEvidenceOverview } from '../../types'
import { sanitizeDiagnostic } from '../../security'
import { copy, type Language } from './text'
const props = defineProps<{ overview: TaskEvidenceOverview | null; language: Language }>()
const emit = defineEmits<{ close: [] }>()
const t = computed(() => copy[props.language])
const safe = (value: string | null | undefined) => sanitizeDiagnostic(value ?? '—')
</script>

<template>
  <aside class="ro-evidence" :aria-label="t.evidence" data-evidence>
    <header><h2>{{ t.evidence }}</h2><button type="button" @click="emit('close')">{{ t.close }}</button></header>
    <p v-if="!overview">{{ t.emptyEvidence }}</p>
    <template v-else>
      <p class="identity">{{ overview.project_id }} / {{ overview.task_id }}<br>{{ t.generation }} {{ overview.result_generation }} · {{ t.round }} {{ overview.pending_round }}</p>
      <section><h3>{{ t.tests }}</h3><p v-if="!overview.tests.length">{{ t.emptyEvidence }}</p>
        <details v-for="item in overview.tests" :key="item.id">
          <summary>{{ item.display_status ?? item.status }} · {{ item.is_adopted ? t.adopted : t.notAdopted }}</summary>
          <p>{{ t.generation }} {{ item.result_generation }} · {{ t.round }} {{ item.round }} · attempt {{ item.result_attempt ?? '—' }}</p>
          <p>{{ item.is_current_generation ? '' : t.historical }} · {{ item.is_current_attempt ? t.currentAttempt : t.otherAttempt }}</p>
          <pre>{{ safe(item.test_command_redacted) }}</pre><p>{{ t.exit }}: {{ item.exit_code ?? '—' }}</p>
          <p v-if="item.reason">{{ safe(item.reason) }}</p><pre>stdout: {{ safe(item.stdout_redacted) }}
stderr: {{ safe(item.stderr_redacted) }}</pre>
        </details>
      </section>
      <section><h3>{{ t.builds }}</h3><p v-if="!overview.builds.length">{{ t.emptyEvidence }}</p>
        <details v-for="item in overview.builds" :key="item.id">
          <summary>{{ item.display_status ?? item.status }} · {{ item.is_adopted ? t.adopted : t.notAdopted }}</summary>
          <p>{{ t.generation }} {{ item.result_generation }} · {{ t.round }} {{ item.round }} · attempt {{ item.result_attempt ?? '—' }}</p>
          <p>{{ item.is_current_generation ? '' : t.historical }} · {{ item.is_current_attempt ? t.currentAttempt : t.otherAttempt }}</p>
          <pre>{{ safe(item.build_command_redacted) }}</pre><p>{{ t.exit }}: {{ item.exit_code ?? '—' }}</p>
          <p v-if="item.reason">{{ safe(item.reason) }}</p><pre>stdout: {{ safe(item.stdout_redacted) }}
stderr: {{ safe(item.stderr_redacted) }}</pre>
        </details>
      </section>
      <section><h3>{{ t.commands }}</h3><p v-if="!overview.commands.length">{{ t.emptyEvidence }}</p>
        <details v-for="item in overview.commands" :key="item.id"><summary>{{ safe(item.command_redacted) }}</summary>
          <p>{{ t.exit }}: {{ item.exit_code ?? '—' }}</p><time>{{ item.started_at ?? '—' }} → {{ item.finished_at ?? '—' }}</time>
          <pre>stdout_ref: {{ safe(item.stdout_ref) }}
stderr_ref: {{ safe(item.stderr_ref) }}</pre>
        </details>
      </section>
      <section><h3>{{ t.changes }}</h3><p v-if="!overview.file_snapshot_available">{{ t.snapshotUnknown }}</p>
        <ul><li v-for="item in overview.current_file_changes ?? []" :key="item.id">{{ item.change_kind }} · {{ item.file_path }}</li></ul>
        <details v-if="overview.file_changes.length"><summary>{{ t.historical }}</summary><p v-for="item in overview.file_changes" :key="item.id">{{ item.file_path }} · {{ item.created_at }}</p></details>
      </section>
      <section><h3>Git</h3><p v-if="!overview.git_events.length">{{ t.emptyEvidence }}</p>
        <p v-for="item in overview.git_events" :key="item.id">{{ item.event_kind }} · {{ item.branch ?? '—' }}<br>{{ item.commit_sha ?? '—' }}<br>{{ item.created_at }}</p>
      </section>
      <section><h3>{{ t.acceptance }}</h3><p v-if="!overview.acceptance_history.length">{{ t.emptyEvidence }}</p>
        <p v-for="item in overview.acceptance_history" :key="item.id">{{ item.accepted ? t.approved : t.rejected }} · {{ t.generation }} {{ item.result_generation }}<br>{{ item.accepted_at }}<br>{{ safe(item.user_note) }}</p>
      </section>
    </template>
  </aside>
</template>

<style scoped>
.ro-evidence { width: 286px; flex: 0 0 286px; min-height: 0; overflow-y: auto; background: #fff; border-left: 1px solid var(--ro-border); padding: 20px; box-sizing: border-box; color: #202b24; }
header { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; } h2 { margin: 0; font-size: 17px; } h3 { font-size: 14px; margin: 0 0 10px; }
button { background: var(--ro-soft); color: var(--ro-accent); border: 1px solid var(--ro-border); padding: 6px 10px; min-height: 32px; border-radius: 6px; cursor: pointer; }
button:focus-visible, summary:focus-visible { outline: 2px solid var(--ro-accent); outline-offset: 2px; }
section { border-top: 1px solid var(--ro-border); padding-top: 18px; margin-top: 18px; } p, time, li { font-size: 12px; line-height: 1.7; overflow-wrap: anywhere; } .identity { color: #526057; }
details { margin: 10px 0; } summary { cursor: pointer; font-size: 13px; overflow-wrap: anywhere; } pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 240px; overflow: auto; padding: 10px; background: var(--ro-bg); font-size: 11px; border-radius: 5px; }
@media (max-width: 720px) { .ro-evidence { position: absolute; right: 0; top: 0; bottom: 0; width: min(88%, 360px); z-index: 2; box-shadow: -6px 0 24px #0002; } }
</style>
