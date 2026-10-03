import { expect, test } from 'claude-code/testing'
import { splitDiff } from '../hooks/diff'
import { runDate, shortPage } from '../hooks/presentation'

function section(path: string, hunks: string): string {
  return `diff --git a/${path} b/${path}\nindex abc..def 100644\n--- a/${path}\n+++ b/${path}\n${hunks}`
}

function hunk(at: number, size: number): string {
  return `@@ -${at} +${at} @@\n-${'a'.repeat(size)}\n+${'b'.repeat(size)}\n`
}

test('splits git show into files and keeps only the commit subject ahead of hunks', () => {
  const first = hunk(1, 3000)
  const second = hunk(5, 3000)
  const source = 'commit abc123\nAuthor: Someone\nDate: today\n\n    Refresh snapshots\n\n    Long explanation.\n\n a.md | 2 +-\n b.md | 2 +-\n 2 files changed\n\n' +
    section('a.md', first) + section('b.md', second)
  expect(source.length).toBeGreaterThan(10000)
  const preview = splitDiff(source)
  expect(preview.subject).toBe('Refresh snapshots')
  expect(preview.files.map(file => file.path)).toEqual(['a.md', 'b.md'])
  expect(preview.files.map(file => file.code)).toEqual([first, second])
  expect(preview.files.map(file => file.fallback)).toEqual(['', ''])
  expect(preview.truncated).toBe(false)
})

test('cuts each Code at a complete hunk boundary and counts every omitted hunk', () => {
  const first = hunk(1, 2800)
  const preview = splitDiff(section('large.md', first + hunk(7, 2800) + hunk(20, 20)))
  expect(preview.files[0]?.code).toBe(first)
  expect(preview.files[0]?.code.length).toBeLessThanOrEqual(10000)
  expect(preview.files[0]?.omittedHunks).toBe(2)
  expect(preview.files[0]?.fallback).toBe('')
  expect(preview.truncated).toBe(true)
})

test('an oversized single hunk is explicitly omitted rather than sliced into invalid Code', () => {
  const preview = splitDiff(section('snapshot.md', hunk(1, 6000)))
  expect(preview.files[0]?.code).toBe('')
  expect(preview.files[0]?.fallback).toBe('')
  expect(preview.files[0]?.omittedHunks).toBe(1)
})

test('recognizes additions, deletions and renames including spaces and a literal a directory', () => {
  const source = 'diff --git a/new.md b/new.md\nnew file mode 100644\n--- /dev/null\n+++ b/new.md\n@@ -0,0 +1 @@\n+new\n' +
    'diff --git a/old.md b/old.md\ndeleted file mode 100644\n--- a/old.md\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n' +
    'diff --git "a/a/old name.md" "b/a/new name.md"\nsimilarity index 100%\nrename from a/old name.md\nrename to a/new name.md\n'
  const preview = splitDiff(source)
  expect(preview.files.map(file => [file.status, file.path, file.oldPath])).toEqual([
    ['A', 'new.md', null], ['D', 'old.md', null], ['R', 'a/new name.md', 'a/old name.md'],
  ])
  expect(preview.files[0]?.code).toMatch('@@ -0,0 +1 @@')
  expect(preview.files[1]?.code).toMatch('@@ -1 +0,0 @@')
  expect(preview.files[2]?.fallback).toMatch('rename to a/new name.md')
})

test('fallback is limited to the malformed file, and incomplete hunks never reach Code', () => {
  const good = hunk(1, 10)
  const preview = splitDiff(section('bad.md', '@@ -1,2 +1,2 @@\n-partial\n') + section('good.md', good))
  expect(preview.files[0]?.code).toBe('')
  expect(preview.files[0]?.fallback).toMatch('partial')
  expect(preview.files[1]?.code).toBe(good)
  expect(preview.files[1]?.fallback).toBe('')
})

test('supports raw unified diffs, no-newline markers, and quoted paths', () => {
  const preview = splitDiff('--- "a/file name.md"\n+++ "b/file name.md"\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n')
  expect(preview.files[0]?.path).toBe('file name.md')
  expect(preview.files[0]?.code).toMatch('\\ No newline at end of file')
  expect(preview.files[0]?.fallback).toBe('')
})

test('bounds the whole drawing to 60 KB while still parsing files after a large commit message', () => {
  const source = 'commit abc\n\n    Subject\n\n    ' + 'body '.repeat(15000) + '\n\n' +
    Array.from({ length: 12 }, (_, index) => section(`${index}.md`, hunk(1, 4000))).join('')
  const preview = splitDiff(source)
  expect(preview.subject).toBe('Subject')
  expect(preview.files[0]?.code.length).toBeGreaterThan(0)
  const rendered = preview.subject + preview.files.map(file => file.path + (file.oldPath ?? '') + file.code + file.fallback).join('')
  expect(new TextEncoder().encode(rendered).length).toBeLessThanOrEqual(60000)
  expect(preview.files.some(file => file.omittedHunks > 0)).toBe(true)
  expect(preview.truncated).toBe(true)
})

test('formats run dates and short page names without changing stored paths', () => {
  expect(runDate('20261003T010203Z')).toBe('10-03 01:02')
  expect(runDate('/runs/2026-10-03T01:02:03Z')).toBe('10-03 01:02')
  expect(runDate(null)).toBe('unknown run')
  expect(shortPage('pages/knowledge/build.md')).toBe('knowledge/build')
})
