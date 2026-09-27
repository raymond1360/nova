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
})
