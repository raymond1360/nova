export function maskKey(value: string): string {
  const trimmed = value.trim()
  if (!trimmed) return ''
  if (trimmed.length <= 8) return '********'
  return `${trimmed.slice(0, 4)}...${trimmed.slice(-4)}`
}

export function sanitizeDiagnostic(value: string): string {
  return value
    .replace(/Authorization:\s*Bearer\s+\S+/gi, 'Authorization: Bearer [REDACTED]')
    .replace(/Cookie:\s*\S+/gi, 'Cookie: [REDACTED]')
    .replace(/sk-[A-Za-z0-9_-]{8,}/g, '[REDACTED_KEY]')
}

export function exportableProviderConfig(provider: { id: string; name: string; base_url: string }) {
  return {
    id: provider.id,
    name: provider.name,
    base_url: provider.base_url,
  }
}
