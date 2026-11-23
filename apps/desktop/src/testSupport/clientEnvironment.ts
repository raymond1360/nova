// The custom renderer needs client SFCs, while keeping Node globals and no browser.
import { builtinEnvironments } from 'vitest/environments'
export default { ...builtinEnvironments.node, name: 'nova-client-renderer', transformMode: 'web' as const }
