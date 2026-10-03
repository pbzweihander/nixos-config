export function shortPage(path: string): string {
  return path.replace(/^pages\//, '').replace(/\.md$/i, '')
}

export function runDate(run: string | null | undefined): string {
  const match = /(\d{4})-?(\d{2})-?(\d{2})[T_ ](\d{2}):?(\d{2})/.exec(run ?? '')
  return match ? `${match[2]}-${match[3]} ${match[4]}:${match[5]}` : 'unknown run'
}

export function clip(text: string, columns: number): string {
  const clean = text.replace(/\s+/g, ' ').trim()
  const points = Array.from(clean)
  const room = Math.max(1, columns)
  return points.length <= room ? clean : points.slice(0, room - 1).join('') + '…'
}

export function rowHotkey(index: number): string | undefined {
  return index < 9 ? String(index + 1) : undefined
}

export function sourceColor(source: string): string {
  if (source === 'remind') return 'warning'
  if (source.startsWith('recall')) return 'permission'
  if (source === 'subagent') return 'claude'
  if (source === 'skill-prompt') return 'success'
  return 'inactive'
}

export function fileColor(status: string): string {
  return status === 'A' ? 'success' : status === 'D' ? 'error' : status === 'R' ? 'permission' : 'warning'
}

export function outcomeColor(message: string): string {
  if (/^(?:Land|Discard): (?:merged|discarded)\b/.test(message)) return 'success'
  if (/^(?:Land|Discard): deferred\b/.test(message)) return 'warning'
  if (/^(?:Landing|Discarding) /.test(message)) return 'permission'
  return 'error'
}
