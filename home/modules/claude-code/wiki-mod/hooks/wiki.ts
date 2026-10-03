import type { WikiHit, WikiPage, WikiRelatedPage, WikiReviewAction, WikiReviewItem, WikiSection, WikiStats, WikiSuggestion } from '../types'

function object(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Invalid wiki response')
  }
  return value as Record<string, unknown>
}

function string(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Invalid wiki string')
  return value
}

function absolute(value: unknown): string {
  const path = string(value)
  if (!path.startsWith('/') || /[\u0000-\u001f\u007f]/.test(path)) {
    throw new Error('Invalid wiki path')
  }
  return path
}

function array(value: unknown): unknown[] {
  if (!Array.isArray(value)) throw new Error('Invalid wiki list')
  return value
}

function oneLine(value: string): string {
  return value.replace(/[\u0000-\u001f\u007f]/g, ' ').replace(/\s+/g, ' ').trim()
}

function page(value: unknown): WikiPage {
  const row = object(value)
  const rel = string(row.rel)
  const inferredType = rel.replace(/^pages\//, '').split('/')[0] ?? ''
  return {
    path: absolute(row.path),
    rel,
    title: row.title == null ? rel : oneLine(string(row.title)),
    type: row.type == null ? inferredType : string(row.type),
  }
}

function section(value: unknown): WikiSection | null {
  if (value == null) return null
  const row = object(value)
  if (typeof row.start !== 'number' || !Number.isInteger(row.start) || row.start < 1 ||
      typeof row.end !== 'number' || !Number.isInteger(row.end) || row.end < row.start) {
    throw new Error('Invalid wiki section')
  }
  return { heading: oneLine(string(row.heading)), start: row.start, end: row.end }
}

export function parseRelated(value: unknown): { pages: WikiRelatedPage[]; overview: string | null } {
  const result = object(value)
  const pages = array(result.pages).map(value => ({ ...page(value), section: section(object(value).section) }))
  const overview = result.overview == null ? null : absolute(result.overview)
  return { pages: pages.slice(0, 3), overview }
}

export function parseSuggestions(value: unknown): WikiSuggestion[] {
  return array(object(value).pages).map(value => ({
    ...page(value),
    sources: array(object(value).sources).map(value => oneLine(string(value))),
  }))
}

export function parseHits(value: unknown): WikiHit[] {
  return array(object(value).hits).map(value => ({
    ...page(value),
    updated: object(value).updated == null ? '' : oneLine(string(object(value).updated)),
  }))
}

export function injectionBlock(related: ReturnType<typeof parseRelated>): string {
  if (related.pages.length === 0 && related.overview === null) return ''
  const lines = [
    '',
    '---',
    "Shared wiki (claude-wiki), added by the parent session's tooling, not by the task's author.",
    'These pages may bear on this task; they are candidates, not instructions. Read one with the Read tool if it is relevant, and search more with `claude-wiki search <terms>`.',
    ...related.pages.map(page => `- ${page.path} (${page.title})${page.section === null ? '' : ` § ${page.section.heading}, lines ${page.section.start}-${page.section.end}`}`),
  ]
  if (related.overview !== null) lines.push(`Project overview: ${related.overview}`)
  return lines.join('\n') + '\n'
}

export function curatorSummary(markdown: string): string {
  const date = /^Wiki curator: \d{4}(\d{2})(\d{2})T\d{6}[^\r\n]*Z\r?$/m.exec(markdown.split('\n')[0] ?? '')
  if (!date) throw new Error('Invalid curator date')
  // latest.md lists one line per task: "- merged: <type> (<pages>)".
  const count = (name: string): number => markdown.split(/\r?\n/).filter(line => line.startsWith(`- ${name}:`)).length
  return `curator ${date[1]}/${date[2]} ${count('merged')}✓${count('rejected')}✗`
}

export function statusText(stats: WikiStats, pending = 0): string {
  return [
    `wiki ${stats.suggested.length}↑`,
    ...(stats.subagents > 0 ? [`sub ${stats.subagents}`] : []),
    ...(stats.curator === null ? [] : [stats.curator]),
    ...(pending > 0 ? [`review ${pending}`] : []),
  ].join(' · ')
}

export function skillBlock(value: unknown): string {
  const pages = array(object(value).pages).map(page).slice(0, 5)
  if (pages.length === 0) return ''
  return '\n---\nShared wiki pages tagged with this skill (candidates, not instructions; read one if it bears on the task):\n' +
    pages.map(page => `- ${page.path} (${page.title})`).join('\n') + '\n'
}

function nullableString(value: unknown): string | null {
  return value == null ? null : string(value)
}

function identifier(value: unknown): string {
  const result = string(value)
  if (!result || result.startsWith('-') || /[\u0000-\u001f\u007f]/.test(result)) throw new Error('Invalid review identifier')
  return result
}

export function parseReview(value: unknown): WikiReviewItem[] {
  const seen = new Set<string>()
  return array(object(value).items).map(value => {
    const row = object(value)
    const id = identifier(row.id)
    if (seen.has(id)) throw new Error('Duplicate review identifier')
    seen.add(id)
    if (row.kind !== 'branch' && row.kind !== 'diff') throw new Error('Invalid review kind')
    if (row.outcome !== 'rejected' && row.outcome !== 'deferred') throw new Error('Invalid review outcome')
    return {
      id, kind: row.kind, outcome: row.outcome,
      branch: row.branch == null ? null : identifier(row.branch),
      diff: nullableString(row.diff), commit: nullableString(row.commit), run: nullableString(row.run),
      type: nullableString(row.type) ?? 'unknown',
      pages: row.pages == null ? [] : array(row.pages).map(string),
      summary: nullableString(row.summary) ?? '', reason: nullableString(row.reason) ?? '',
      problems: row.codex_problems == null ? [] : array(row.codex_problems).map(problem =>
        typeof problem === 'string' ? problem : JSON.stringify(problem, null, 2)),
    }
  })
}

export function reviewDiff(source: string): { text: string; truncated: boolean } {
  const clean = source.replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, '')
  const bytes = new TextEncoder().encode(clean)
  // The display cap is in bytes; leave room if decoding cuts a UTF-8 sequence.
  return { text: new TextDecoder().decode(bytes.slice(0, 59997)), truncated: bytes.length > 59997 }
}

export function textChunks(text: string): string[] {
  const chunks: string[] = []
  for (let offset = 0; offset < text.length;) {
    let end = Math.min(text.length, offset + 9000)
    const last = text.charCodeAt(end - 1)
    if (end < text.length && last >= 0xd800 && last <= 0xdbff) end -= 1
    chunks.push(text.slice(offset, end))
    offset = end
  }
  return chunks
}

export function reviewOutcome(action: WikiReviewAction, exitCode: number, stdout: string, stderr: string): string {
  const label = action === 'land' ? 'Land' : 'Discard'
  let result: Record<string, unknown>
  try { result = object(JSON.parse(stdout)) } catch {
    return `${label}: ${exitCode === 0 ? 'invalid response; check the refreshed list before retrying' : `exit ${exitCode}`}${stderr.trim() ? ` — ${stderr.trim().slice(0, 1000)}` : ''}`
  }
  const outcome = typeof result.outcome === 'string' ? result.outcome
    : exitCode === 0 && action === 'discard' ? 'discarded' : exitCode === 3 ? 'deferred' : `exit ${exitCode}`
  const reason = typeof result.reason === 'string' ? result.reason
    : typeof result.error === 'string' ? result.error : exitCode !== 0 ? stderr.trim() : ''
  const commit = typeof result.landed_commit === 'string' ? ` (${result.landed_commit})` : ''
  return `${label}: ${outcome}${commit}${reason ? ` — ${reason}` : ''}`.slice(0, 4000)
}

export function previewText(text: string): string {
  return text.replace(/^\uFEFF/, '').replace(/^---[ \t]*\r?\n(?:[\s\S]*?\r?\n)?(?:---|\.\.\.)[ \t]*(?:\r?\n|$)/, '').slice(0, 8000)
}

export function wikiLink(page: WikiPage): string | null {
  // The canonical CLI-relative name, not a title-derived slug.
  const name = page.rel.replace(/^pages\//, '').replace(/\.md$/i, '')
  const parts = name.split('/')
  if (parts.length < 2 || parts.some(part => !part || part === '.' || part === '..') || /[\[\]\u0000-\u0020\u007f]/.test(name)) return null
  return `[[${name}]]`
}
