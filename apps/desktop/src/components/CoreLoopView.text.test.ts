import { describe, expect, it } from 'vitest'
// v0.1.0 scope correction (Work Package 2, required test coverage item 5):
// there is no `@vue/test-utils`-based component test harness set up in this
// project (see `package.json`), so this reads the real `.vue` source text
// directly (via Vite's built-in `?raw` import, already declared by
// `vite/client` — no `@types/node` needed) and asserts on it — a
// lightweight but real regression guard against the forced dual-AI pairing
// copy the Codex scope-correction audit flagged
// (`CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`, Blocker 3: "Codex 开发 /
// Claude 审核" / "Claude 开发 / Codex 审核" dropdown labels, and a start gate
// that read `review_ready`). Asserting on the source text is meaningful here
// specifically because the forbidden strings are hardcoded template
// literals, not something that could only be observed by actually mounting
// and rendering the component.
import source from './CoreLoopView.vue?raw'

describe('CoreLoopView.vue single-AI copy', () => {
  it('never presents the developer AI selector as a forced dev/reviewer pairing', () => {
    expect(source).not.toContain('Codex 开发 / Claude 审核')
    expect(source).not.toContain('Claude 开发 / Codex 审核')
  })

  it('offers Codex and Claude Code as plain, independent choices', () => {
    expect(source).toContain('>Codex</option>')
    expect(source).toContain('>Claude Code</option>')
  })

  it('never gates the start button on the unselected AIs review_ready', () => {
    // The real gate lives in `coreLoop.ts`'s `canStart` getter (covered by
    // its own store tests); this only guards against a *second*,
    // duplicate check being reintroduced directly in the template.
    expect(source).not.toMatch(/disabled="[^"]*review_ready/)
  })

  it('frames the reviewer readiness panel as optional and non-blocking', () => {
    expect(source).toContain('可选诊断，不影响启动')
  })

  it('frames historical review records as an optional diagnostic, not a required verdict section', () => {
    expect(source).toContain('可选诊断信息')
  })

  it('renders the no-build confirmation prompt gated on the real backend query', () => {
    expect(source).toContain('store.noBuildConfirmationNeeded')
    expect(source).toContain('handleConfirmNoBuild')
  })

  it('disables the no-build confirm button and input while a request is in flight', () => {
    // v0.1.0 scope correction (Work Package 2 remediation, Minor 1).
    expect(source).toContain(':disabled="store.confirmingNoBuild"')
  })

  it('renders the no-tests confirmation prompt gated on the real backend query', () => {
    // Work Package 3 final review remediation (Major 1).
    expect(source).toContain('store.noTestsConfirmationNeeded')
    expect(source).toContain('handleConfirmNoTests')
  })

  it('disables the no-tests confirm button and input while a request is in flight', () => {
    // Work Package 3 final review remediation (Major 1).
    expect(source).toContain(':disabled="store.confirmingNoTests"')
  })

  it('shows the no-tests recovery card instead of the generic needs_user_action message whenever the backend needs it', () => {
    // Phase 3 final rereview remediation (Case C visibility): there is no
    // `@vue/test-utils`-based mount harness in this project (see the
    // module doc comment above), so this cannot render the component and
    // assert on the actually-visible DOM for a Case C store state.
    // Instead, this asserts on the real template source that the generic
    // "needs_user_action" fallback message's own `v-if` condition
    // explicitly excludes `store.noTestsConfirmationNeeded` — the same
    // real property `store.noTestsConfirmationNeeded` the no-tests card
    // above is gated on. Since the backend fix makes that property `true`
    // for Case C exactly like it always has for the original
    // awaiting-confirmation state, this structural guarantee is sufficient
    // to prove the recovery card, not the generic message, is what renders
    // whenever the backend says a no-tests entry point (first-time or
    // Case C recovery) is needed — real component-mount coverage remains a
    // known gap, tracked as a Minor, not fixed in this round.
    const genericMessageMatch = source.match(
      /<p\s+v-if="store\.currentTask\.state === 'needs_user_action'[^"]*"/,
    )
    expect(genericMessageMatch).not.toBeNull()
    expect(genericMessageMatch?.[0]).toContain('!store.noTestsConfirmationNeeded')
  })

  // Phase 4 (evidence and document gate): source-text checks only — there
  // is still no `@vue/test-utils` mount harness in this project (see the
  // module doc comment above), so these cannot render the component and
  // inspect the actually-visible DOM. Real component-mount coverage
  // remains a known, documented gap (tracked as a Minor), not fixed in
  // this round.

  it('never introduces v0.2.0 dual-AI review product text in the new evidence/document sections', () => {
    // The existing reviews-diagnostic section already carries this label;
    // this guards that no *new* text repeats "Codex/Claude 审核" as a
    // product-facing verb for Nova's own verification.
    expect(source).not.toContain('Codex 审核')
    expect(source).not.toContain('Claude 审核')
    expect(source).not.toContain('首次审核')
    expect(source).not.toContain('二次审核')
    expect(source).toContain('Nova 验证')
  })

  it('gates the acceptance-readiness text purely on the real backend field, never a local computation', () => {
    expect(source).toContain('store.evidenceOverview.acceptance_ready')
  })

  it('renders the timeline and technical evidence detail behind collapsed <details>, not expanded by default', () => {
    expect(source).toMatch(/<details class="timeline-detail">/)
    expect(source).toMatch(/<details class="evidence-detail">/)
  })

  it('never renders raw stdout/stderr content in the evidence technical details', () => {
    expect(source).not.toContain('stdout_ref')
    expect(source).not.toContain('stderr_ref')
  })

  it('never binds a numeric percent-complete value in the template', () => {
    const templateMatch = source.match(/<template>[\s\S]*<\/template>/)
    expect(templateMatch).not.toBeNull()
    // No interpolation ending in a literal `%` (a fabricated "N% done"
    // readout) anywhere in the rendered markup.
    expect(templateMatch?.[0]).not.toMatch(/\{\{[^}]*\}\}\s*%/)
  })

  it('lists task documents by their real display name and only ever passes document_type back to the store', () => {
    expect(source).toContain('doc.display_name')
    expect(source).toContain('handleViewDocument(doc.document_type)')
    // No free-text input feeding into `viewTaskDocument`/`read_task_document`.
    expect(source).not.toMatch(/viewTaskDocument\(\s*[a-zA-Z]+\.value/)
  })

  it('shows evidence and documents regardless of task state, including terminal ones', () => {
    const evidenceSection = source.match(
      /<section v-if="store\.currentTask" class="block">\s*<h3>7\. 时间线与 Nova 验证<\/h3>/,
    )
    const documentSection = source.match(
      /<section v-if="store\.currentTask" class="block">\s*<h3>8\. 任务文档<\/h3>/,
    )
    expect(evidenceSection).not.toBeNull()
    expect(documentSection).not.toBeNull()
  })
})


describe('WP2A remediation user entry points', () => {
  it('shows the original four requirement fields before the explicit snapshot action', () => {
    for (const field of ['goal', 'in_scope', 'not_in_scope', 'done_criteria']) expect(source).toContain(`store.currentTask.${field}`)
    expect(source).toContain('确认沿用，生成需求清单')
    expect(source).toContain('@click="store.generateRequirementSnapshot"')
  })
  it('keeps result viewing and rejection available but gates acceptance on submission precheck', () => {
    expect(source).toContain(':disabled="!store.completionPrecheck?.can_submit_acceptance"')
    expect(source).toContain('store.acceptResult(false, acceptNote)')
    expect(source).toContain('handleViewDocument')
    expect(source).toContain('当前结果尚不满足提交验收条件')
  })
  it('makes an explicitly resumed queued task startable without an automatic AI call', () => {
    expect(source).toContain("['confirmed', 'queued'].includes(store.currentTask.state)")
    expect(source).toContain('store.noBuildConfirmationNeeded')
    expect(source).toContain('handleConfirmNoBuild')
  })
  it('only labels evidence with both generation and dev-attempt match as current', () => {
    expect(source).toContain('t.is_current_generation && t.is_current_attempt')
    expect(source).toContain('b.is_current_generation && b.is_current_attempt')
  })
})
