import type { WikiDiffFile, WikiDiffPreview } from '../types'

const CODE_LIMIT = 10000
const PREVIEW_BYTES = 60000
const encoder = new TextEncoder()

function bytes(text: string): number { return encoder.encode(text).length }

function cutBytes(text: string, limit: number): string {
  if (bytes(text) <= limit) return text
  // Trim an incomplete UTF-8 tail rather than drawing a replacement character.
  return new TextDecoder().decode(encoder.encode(text).slice(0, Math.max(0, limit))).replace(/\ufffd$/, '')
}

function gitPath(text: string, prefixed = true): string {
  let path = text.trim()
  if (path.startsWith('"') && path.endsWith('"')) {
    try { path = JSON.parse(path) as string } catch { path = path.slice(1, -1) }
  }
  return prefixed ? path.replace(/^[ab]\//, '') : path
}

function fileIdentity(section: string): Pick<WikiDiffFile, 'path' | 'oldPath' | 'status'> {
  const from = /^rename from (.+)$/m.exec(section)?.[1]
  const to = /^rename to (.+)$/m.exec(section)?.[1]
  const old = /^--- (.+)$/m.exec(section)?.[1]
  const next = /^\+\+\+ (.+)$/m.exec(section)?.[1]
  const pair = /^diff --git ("(?:\\.|[^"\\])*"|a\/.*?) ("(?:\\.|[^"\\])*"|b\/.*?)$/m.exec(section)
  const deleted = /^deleted file mode /m.test(section) || next === '/dev/null'
  const added = /^new file mode /m.test(section) || old === '/dev/null'
  const path = to ? gitPath(to, false) : gitPath((deleted ? old : next) ?? pair?.[2] ?? pair?.[1] ?? 'diff')
  return { path, oldPath: from ? gitPath(from, false) : null, status: to ? 'R' : added ? 'A' : deleted ? 'D' : 'M' }
}

// Validate counts, so a partial final hunk never silently becomes uncolored Code.
function hunksOf(section: string): string[] | null {
  const lines = section.split('\n')
  const first = lines.findIndex(line => line.startsWith('@@ '))
  if (first < 0) return null
  const hunks: string[] = []
  let index = first
  while (index < lines.length) {
    if (lines[index] === '') { index += 1; continue }
    const header = /^@@ -\d+(?:,(\d+))? \+\d+(?:,(\d+))? @@.*$/.exec(lines[index] ?? '')
    if (!header) return null
    let old = Number(header[1] ?? 1)
    let next = Number(header[2] ?? 1)
    const start = index++
    while (old > 0 || next > 0) {
      const line = lines[index]
      if (line === undefined) return null
      if (line.startsWith('\\ No newline at end of file')) { index += 1; continue }
      if (line.startsWith(' ')) { old -= 1; next -= 1 }
      else if (line.startsWith('-')) old -= 1
      else if (line.startsWith('+')) next -= 1
      else return null
      if (old < 0 || next < 0) return null
      index += 1
    }
    if (lines[index]?.startsWith('\\ No newline at end of file')) index += 1
    hunks.push(lines.slice(start, index).join('\n') + '\n')
  }
  return hunks
}

/** Prepare a bounded drawing from the complete CLI response, not a sliced hunk. */
export function splitDiff(source: string): WikiDiffPreview {
  const clean = source.replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, '')
  const firstFile = clean.search(/^diff --git /m)
  const header = firstFile >= 0 ? clean.slice(0, firstFile) : ''
  const subject = cutBytes((/^ {4}(\S.*)$/m.exec(header)?.[1] ?? /^Subject: (.+)$/m.exec(header)?.[1] ?? '').trim(), 500)
  const sections = firstFile >= 0 ? clean.slice(firstFile).split(/(?=^diff --git )/m) : clean.trim() ? [clean] : []
  const preview: WikiDiffPreview = { subject, files: [], omittedFiles: 0, truncated: false }
  let remaining = PREVIEW_BYTES - bytes(subject)
  for (const [index, section] of sections.entries()) {
    const identity = fileIdentity(section)
    const headerBytes = bytes(identity.path) + bytes(identity.oldPath ?? '') + 16
    if (remaining < headerBytes) {
      preview.omittedFiles = sections.length - index
      preview.truncated = true
      break
    }
    remaining -= headerBytes
    const file: WikiDiffFile = { ...identity, code: '', fallback: '', omittedHunks: 0 }
    const hunks = hunksOf(section)
    if (hunks === null) {
      file.fallback = cutBytes(section.trim(), remaining)
      preview.truncated ||= bytes(file.fallback) < bytes(section.trim())
      remaining -= bytes(file.fallback)
    } else {
      for (const [hunkIndex, hunk] of hunks.entries()) {
        if (file.code.length + hunk.length > CODE_LIMIT || bytes(hunk) > remaining) {
          file.omittedHunks = hunks.length - hunkIndex
          preview.truncated = true
          break
        }
        file.code += hunk
        remaining -= bytes(hunk)
      }
    }
    preview.files.push(file)
  }
  return preview
}
