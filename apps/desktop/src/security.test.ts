import { describe, expect, it } from 'vitest'
import { exportableProviderConfig, maskKey, sanitizeDiagnostic } from './security'

describe('frontend security helpers', () => {
  it('masks api keys without exposing the middle', () => {
    const fakeKey = ['sk', 'abcdefghijklmnopqrstuvwxyz'].join('-')
    expect(maskKey(fakeKey)).toBe('sk-a...wxyz')
    expect(maskKey('short')).toBe('********')
  })

  it('sanitizes auth diagnostics', () => {
    const firstKey = ['sk', 'secretvalue'].join('-')
    const secondKey = ['sk', 'anothersecret'].join('-')
    const cleaned = sanitizeDiagnostic(`Authorization: Bearer ${firstKey} Cookie: sid=abc ${secondKey}`)
    expect(cleaned).not.toContain(firstKey)
    expect(cleaned).not.toContain('sid=abc')
  })

  it('exports provider config without api key', () => {
    const exported = exportableProviderConfig({
      id: 'sdkmax',
      name: 'SDKMAX',
      base_url: 'https://api.sdkmax.com',
    })
    expect(JSON.stringify(exported)).not.toContain('api_key')
    expect(exported.base_url).toBe('https://api.sdkmax.com')
  })
})
