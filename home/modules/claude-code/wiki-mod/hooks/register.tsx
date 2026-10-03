import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'
import type { WikiPane, WikiReview, WikiReviewAction, WikiReviewConfirmation, WikiStats } from '../types'
import { curatorSummary, injectionBlock, parseHits, parseRelated, parseReview, parseSuggestions, previewText, reviewDiff, reviewOutcome, skillBlock, statusText, textChunks, wikiLink } from './wiki'
import { splitDiff } from './diff'
import { clip, fileColor, outcomeColor, rowHotkey, runDate, shortPage, sourceColor } from './presentation'

const PANE = 'wiki'
const REVIEW_PANE = 'wiki-review'
const stats = atom({ plugin: 'wiki-mod', key: 'stats' } as const, {
  suggested: [], subagents: 0, curator: null, refreshId: 0,
} satisfies WikiStats)
const pane = atom({ plugin: 'wiki-mod', key: 'pane' } as const, {
  query: '', hits: [], searching: false, searched: false, searchId: 0, searchError: '',
  selected: null, preview: '', loading: false, selectionId: 0, previewError: '', actionError: '',
} satisfies WikiPane)
const reviewRef = { plugin: 'wiki-mod', key: 'review' } as const
const review = atom(reviewRef, {
  items: [], listRequest: 0, listLoading: false, listError: '',
  selectedId: null, selectionRequest: 0, diff: '', diffLoading: false, diffTruncated: false, diffError: '',
  confirmation: null, actionToken: 0, operation: null, message: '',
} satisfies WikiReview)

// UI handlers and background refreshes must also absorb expected host refusals.
async function quiet(work: () => Promise<unknown>): Promise<void> {
  try { await work() } catch { /* Keep the current drawing and let the session continue. */ }
}

// The engine scanner follows $ only within this module. Keep host calls here.
async function runJson($: EngineInterface, bin: string, args: string[], stdin?: string): Promise<unknown> {
  const result = await $.process.run([bin, ...args], { timeoutMs: 4000, ...(stdin === undefined ? {} : { stdin }) })
  if (result.exitCode !== 0 || result.isStdoutTruncated) throw new Error('Wiki command failed')
  return JSON.parse(result.stdout) as unknown
}

async function refreshReview($: EngineInterface, curatorBin: string): Promise<void> {
  await quiet(async () => {
    const { listRequest } = await update($, review, value => ({ ...value, listRequest: value.listRequest + 1, listLoading: true }))
    try {
      const items = parseReview(await runJson($, curatorBin, ['review', '--json']))
      await update($, review, value => {
        if (value.listRequest !== listRequest) return value
        const before = value.items.find(item => item.id === value.selectedId)
        const after = items.find(item => item.id === value.selectedId)
        const same = before !== undefined && after !== undefined && JSON.stringify(before) === JSON.stringify(after)
        return {
          ...value, items, listLoading: false, listError: '',
          ...(same ? {} : { selectedId: null, diff: '', diffView: undefined, diffError: '', diffLoading: false, diffTruncated: false, selectionRequest: value.selectionRequest + 1 }),
          confirmation: same ? value.confirmation : null,
        }
      })
    } catch {
      await update($, review, value => value.listRequest === listRequest
        ? { ...value, listLoading: false, listError: 'Curator review is unavailable. Try Refresh.', confirmation: null } : value)
    }
  })
}

async function refresh($: EngineInterface, bin: string, curatorBin: string): Promise<void> {
  await refreshReview($, curatorBin)
  await quiet(async () => {
    const { refreshId } = await update($, stats, current => ({ ...current, refreshId: current.refreshId + 1 }))
    const suggested = parseSuggestions(await runJson($, bin, ['injections', '--session', await $.session.id(), '--json']))
    const home = await $.env.get('HOME')
    let curator: string | null = null
    if (home) {
      const path = `${home.replace(/\/$/, '')}/.cache/wiki-curator/latest.md`
      if (await $.fs.exists(path)) curator = curatorSummary(await $.fs.read(path))
    }
    const current = await update($, stats, current => current.refreshId === refreshId
      ? { ...current, suggested, curator } : current)
    if (current.refreshId === refreshId) $.ui.status(statusText(current, (await read($, review)).items.length))
  })
}

async function openReview($: EngineInterface, bin: string, curatorBin: string): Promise<string> {
  try {
    await update($, review, value => ({ ...value, confirmation: null }))
    const opened = await $.ui.open({ id: REVIEW_PANE, title: 'Curator review', focus: true, closeOnEscape: true })
    await refresh($, bin, curatorBin)
    return opened.isPlaced ? 'Curator review pane opened.' : 'Curator review pane could not be displayed here.'
  } catch {
    return 'Curator review pane is unavailable.'
  }
}

async function closeReview($: EngineInterface): Promise<void> {
  await quiet(async () => {
    // Calls on $ skip this plugin's own hooks; clear before closing explicitly.
    await update($, review, value => ({ ...value, confirmation: null }))
    await $.ui.close({ id: REVIEW_PANE })
  })
}

async function selectReview($: EngineInterface, curatorBin: string, id: string): Promise<void> {
  await quiet(async () => {
    const selected = await update($, review, value => value.operation || !value.items.some(item => item.id === id) ? value : {
      ...value, selectedId: id, selectionRequest: value.selectionRequest + 1, confirmation: null,
      diff: '', diffView: undefined, diffLoading: true, diffTruncated: false, diffError: '', message: '',
    })
    if (selected.operation || selected.selectedId !== id) return
    const { selectionRequest } = selected
    try {
      const result = await $.process.run([curatorBin, 'show', id], { timeoutMs: 4000 })
      if (result.exitCode !== 0 || result.isStdoutTruncated) throw new Error('Diff unavailable')
      const preview = reviewDiff(result.stdout)
      const diffView = splitDiff(result.stdout)
      await update($, review, value => value.selectionRequest === selectionRequest
        ? { ...value, diff: preview.text, diffView, diffTruncated: diffView.truncated, diffLoading: false } : value)
    } catch {
      await update($, review, value => value.selectionRequest === selectionRequest
        ? { ...value, diffLoading: false, diffError: 'Could not load the diff. Select the item to retry.' } : value)
    }
  })
}

async function armReview($: EngineInterface, id: string, action: WikiReviewAction): Promise<void> {
  await quiet(() => update($, review, value => {
    const item = value.items.find(item => item.id === id)
    if (!item || value.selectedId !== id || value.operation || value.listLoading || value.listError ||
        value.diffLoading || (action === 'land' && item.kind !== 'branch')) return value
    return {
      ...value, actionToken: value.actionToken + 1, message: '',
      confirmation: { id, action, revision: JSON.stringify(item), token: value.actionToken + 1 },
    }
  }))
}

async function confirmReview($: EngineInterface, bin: string, curatorBin: string, expected: WikiReviewConfirmation): Promise<void> {
  await quiet(async () => {
    let target: string
    // Consume this exact confirmation atomically before any process can start.
    // Another press, selection change, refresh, or reload cannot reuse it.
    for (;;) {
      const { value, version } = await $.state.get(reviewRef)
      if (!value || value.operation || value.listLoading || value.listError || value.selectedId !== expected.id ||
          JSON.stringify(value.confirmation) !== JSON.stringify(expected)) return
      const item = value.items.find(item => item.id === expected.id)
      if (!item || JSON.stringify(item) !== expected.revision || (expected.action === 'land' && item.kind !== 'branch')) return
      target = expected.action === 'land' ? item.branch ?? item.id : item.id
      const claimed = await $.state.set(reviewRef, {
        ...value, confirmation: null, operation: expected, message: `${expected.action === 'land' ? 'Landing' : 'Discarding'} ${item.id}…`,
      }, { ifVersion: version })
      if (claimed.isSet) break
    }
    let message: string
    try {
      const result = await $.process.run([curatorBin, expected.action, target, '--json'], {
        timeoutMs: expected.action === 'land' ? 150000 : 4000,
      })
      message = result.isStdoutTruncated ? 'Curator response was truncated; check the refreshed list before retrying.'
        : reviewOutcome(expected.action, result.exitCode, result.stdout, result.stderr)
    } catch {
      message = 'Curator action failed or timed out. Check the refreshed list before retrying.'
    }
    await update($, review, value => ({ ...value, message }))
    // Keep mutation controls unavailable until both the list and status refresh finish.
    try { await refresh($, bin, curatorBin) } finally {
      await update($, review, value => ({ ...value, operation: null }))
    }
  })
}

async function selectPage($: EngineInterface, path: string): Promise<void> {
  await quiet(async () => {
    const current = await read($, pane)
    const session = await read($, stats)
    const selected = current.hits.find(page => page.path === path) ?? session.suggested.find(page => page.path === path)
    if (!selected) return
    const { selectionId } = await update($, pane, value => ({
      ...value, selected, preview: '', loading: true, previewError: '', actionError: '', selectionId: value.selectionId + 1,
    }))
    try {
      const preview = previewText(await $.fs.read(selected.path))
      await update($, pane, value => value.selectionId === selectionId ? { ...value, preview, loading: false } : value)
    } catch {
      await update($, pane, value => value.selectionId === selectionId
        ? { ...value, loading: false, previewError: 'Could not read this page.' } : value)
    }
  })
}

async function search($: EngineInterface, bin: string, query: string): Promise<void> {
  await quiet(async () => {
    const words = query.trim().split(/\s+/).filter(Boolean)
    const { searchId } = await update($, pane, value => ({
      ...value, query, searchId: value.searchId + 1, searching: words.length > 0, searched: words.length > 0, hits: [], searchError: '',
    }))
    if (words.length === 0) return
    try {
      // End option parsing so user-entered terms cannot become CLI flags.
      const hits = parseHits(await runJson($, bin, ['search', '--json', '--', ...words]))
      await update($, pane, value => value.searchId === searchId ? { ...value, hits, searching: false } : value)
    } catch {
      await update($, pane, value => value.searchId === searchId
        ? { ...value, searching: false, searchError: 'Wiki search is unavailable. Try again.' } : value)
    }
  })
}

async function insert($: EngineInterface, kind: 'path' | 'link'): Promise<void> {
  await quiet(async () => {
    const { selected } = await read($, pane)
    if (!selected) return
    const text = kind === 'path' ? selected.path : wikiLink(selected)
    if (text === null) {
      await update($, pane, value => ({ ...value, actionError: 'This page has no canonical wiki link.' }))
      return
    }
    try {
      const filled = await $.prompt.fill({ text, mode: 'insert' })
      await update($, pane, value => ({ ...value, actionError: filled.isFilled ? '' : 'The prompt could not accept the insertion.' }))
    } catch {
      await update($, pane, value => ({ ...value, actionError: 'The prompt could not accept the insertion.' }))
    }
  })
}

export const register: Register = (on, options) => {
  const bin = typeof options.claudeWikiBin === 'string' && options.claudeWikiBin.trim() ? options.claudeWikiBin : 'claude-wiki'
  const curatorBin = typeof options.wikiCuratorBin === 'string' && options.wikiCuratorBin.trim() ? options.wikiCuratorBin : 'wiki-curator'

  on('agent.spawn', async ($, e, next) => {
    if (e.fork) return next(e)
    let block = ''
    try {
      const cwd = await $.session.cwd()
      const session = await $.session.id()
      block = injectionBlock(parseRelated(await runJson($, bin,
        ['related', '--json', '-n', '3', '--cwd', cwd, '--session', session, '--source', 'subagent'], e.prompt.slice(0, 8000))))
    } catch {
      return next(e)
    }
    if (!block) return next(e)
    await quiet(async () => {
      const current = await update($, stats, value => ({ ...value, subagents: value.subagents + 1 }))
      $.ui.status(statusText(current, (await read($, review)).items.length))
    })
    // Keep continuation outside the failure handler: never start an agent twice.
    return next({ ...e, prompt: e.prompt + block })
  })

  on('skill.prompt', async ($, e, next) => {
    if (e.skill.split(':').at(-1) === 'claude-wiki') return next(e)
    let block: string
    try {
      block = skillBlock(await runJson($, bin, [
        'skill-pages', e.skill, '--json', '-n', '5', '--cwd', await $.session.cwd(),
        '--session', await $.session.id(), '--source', 'skill-prompt',
      ]))
    } catch {
      return next(e)
    }
    return next(block ? { ...e, text: e.text + block } : e)
  })

  on('session.start', async ($, e, next) => {
    await quiet(() => $.command.register({ name: 'wiki', description: 'Browse shared wiki suggestions and search pages' }))
    await quiet(() => $.command.register({ name: 'wiki-review', description: 'Review, land, or discard curator proposals' }))
    await quiet(() => update($, review, value => ({
      ...value, confirmation: null, operation: null, listLoading: false, diffLoading: false,
      selectionRequest: value.selectionRequest + 1,
      message: value.operation ? 'An action was interrupted by reload. Check the refreshed list before retrying.' : value.message,
    })))
    await refresh($, bin, curatorBin)
    $.clock.every(5 * 60 * 1000, () => refresh($, bin, curatorBin))
    return next(e)
  })

  on('turn.complete', async ($, e, next) => {
    const result = await next(e)
    if (e.agentId === undefined) await refresh($, bin, curatorBin)
    return result
  })

  on('command.run', { command: 'wiki' }, async $ => {
    try {
      await refresh($, bin, curatorBin)
      const opened = await $.ui.open({ id: PANE, title: 'Shared wiki', focus: true, closeOnEscape: true })
      return { text: opened.isPlaced ? 'Wiki pane opened.' : 'Wiki pane could not be displayed here.' }
    } catch {
      return { text: 'Wiki pane is unavailable.' }
    }
  })

  on('command.run', { command: 'wiki-review' }, async $ => ({ text: await openReview($, bin, curatorBin) }))

  on('ui.close', { id: REVIEW_PANE }, async ($, e, next) => {
    await quiet(() => update($, review, value => ({ ...value, confirmation: null })))
    return next(e)
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e, next) => {
    if (e.surface !== 'terminal' && e.surface !== 'desktop' && e.surface !== 'vscode') return next(e)
    try {
      const { Box, Text, Button, Input, Markdown } = $.ui.resolve(e)
      const session = await read($, stats)
      const view = await read($, pane)
      const columns = Math.max(1, e.props.bodyColumns)
      return (
        <Box key="wiki-body" flexDirection="column" width={columns} gap={1}>
          <Box key="wiki-header" flexDirection="row" flexWrap="wrap" gap={1}>
            <Text bold color="claude">Shared wiki</Text>
            <Text color="permission" inverse> {session.suggested.length} suggested </Text>
            {view.searched && <Text color="success" inverse> {view.hits.length} matches </Text>}
            {session.curator && <Text dimColor>{session.curator}</Text>}
          </Box>
          <Box flexDirection="column">
            <Text bold color="permission">Suggested in this session</Text>
            {session.suggested.length === 0 && <Text dimColor>No suggestions yet.</Text>}
            {session.suggested.map((page, index) => (
              <Box key={`suggested-row-${index}`} flexDirection="row" flexWrap="wrap" gap={1}>
                <Text color="permission" bold={view.selected?.path === page.path}>{view.selected?.path === page.path ? '▶' : ' '}</Text>
                <Button key={`suggested-${index}`} label={String(index + 1)} plain hotkey={rowHotkey(index)}
                  onPress={() => selectPage($, page.path)} />
                <Box minWidth={0} flexShrink={1}>
                  <Text bold={view.selected?.path === page.path} wrap="truncate-end">{clip(page.title, columns - 9)}</Text>
                </Box>
                {page.sources.map(source => <Text color={sourceColor(source)} inverse> {source} </Text>)}
              </Box>
            ))}
          </Box>
          <Box key="wiki-search" flexDirection="column" borderStyle="round" borderColor="inactive" paddingX={1}>
            <Text bold color="permission">Search</Text>
            <Input key="search" label="Search wiki" placeholder="Search terms" value={view.query} submitLabel="search" autoFocus
              onInput={query => quiet(() => update($, pane, value => ({ ...value, query })))}
              onSubmit={query => search($, bin, query)} />
            {view.searching && <Text dimColor>Searching…</Text>}
            {view.searchError && <Text color="error">{view.searchError}</Text>}
            {view.searched && !view.searching && !view.searchError && view.hits.length === 0 && <Text dimColor>No results.</Text>}
            {view.hits.map((page, index) => (
              <Box key={`result-row-${index}`} flexDirection="row" gap={1}>
                <Text color="permission" bold={view.selected?.path === page.path}>{view.selected?.path === page.path ? '▶' : ' '}</Text>
                <Button key={`result-${index}`} label={String(session.suggested.length + index + 1)} plain
                  hotkey={rowHotkey(session.suggested.length + index)} onPress={() => selectPage($, page.path)} />
                <Box minWidth={0} flexGrow={1} flexShrink={1}>
                  <Text bold={view.selected?.path === page.path} wrap="truncate-end">{clip(page.title, columns - 28)}</Text>
                </Box>
                <Text color="permission" wrap="truncate-end">{clip(page.type, 12)}</Text>
                <Text dimColor>{page.updated.slice(0, 10) || 'unknown date'}</Text>
              </Box>
            ))}
          </Box>
          <Box key="wiki-preview" flexDirection="column" borderStyle="round" borderColor="permission" paddingX={1} gap={1}>
            <Text bold color="permission">{view.selected?.title || 'Preview'}</Text>
            {view.selected === null && <Text dimColor>Select a page to preview it.</Text>}
            {view.loading && <Text dimColor>Loading…</Text>}
            {view.previewError && <Text color="error">{view.previewError}</Text>}
            {view.preview && <Markdown key="preview" text={view.preview} />}
          </Box>
          {view.actionError && <Text color="error">{view.actionError}</Text>}
          <Box key="wiki-actions" flexDirection="row" flexWrap="wrap" gap={1}>
            {view.selected !== null && <Button key="insert-path" label="Insert path" variant="primary" hotkey="p" onPress={() => insert($, 'path')} />}
            {view.selected !== null && <Button key="insert-link" label="Insert link" hotkey="i" onPress={() => insert($, 'link')} />}
            <Button key="review-curator" label="Review curator" hotkey="r" onPress={() => openReview($, bin, curatorBin)} />
            <Button key="close" label="Close" role="dismiss" onPress={() => quiet(() => $.ui.close({ id: PANE }))} />
          </Box>
        </Box>
      )
    } catch {
      return next(e)
    }
  })

  on('ui.render', { component: 'Pane', requestId: REVIEW_PANE }, async ($, e, next) => {
    try {
      const { Box, Text, Button, Code } = $.ui.resolve(e)
      const view = await read($, review)
      const session = await read($, stats)
      const columns = Math.max(1, e.props.bodyColumns)
      const selected = view.items.find(item => item.id === view.selectedId)
      const confirmation = view.confirmation
      const rejected = view.items.filter(item => item.outcome === 'rejected').length
      const deferred = view.items.length - rejected
      const selectedColor = selected?.outcome === 'rejected' ? 'error' : 'warning'
      const canAct = selected && !view.operation && !view.listLoading && !view.listError && !view.diffLoading && !confirmation
      // Old state can survive a hot reload; derive its view until the next selection.
      const diff = view.diffView ?? splitDiff(view.diff)
      return (
        <Box key="review-body" flexDirection="column" width={columns} gap={1}>
          <Box key="review-header" flexDirection="row" flexWrap="wrap" gap={1}>
            <Text bold color="claude">Curator review</Text>
            <Text color="error" inverse> {rejected} rejected </Text>
            <Text color="warning" inverse> {deferred} deferred </Text>
            {session.curator && <Text dimColor>{session.curator}</Text>}
          </Box>
          <Box key="review-list" flexDirection="column">
            {view.listLoading && <Text dimColor>Refreshing…</Text>}
            {view.listError && <Text color="error">{view.listError}</Text>}
            {!view.listLoading && !view.listError && view.items.length === 0 && <Text dimColor>No pending curator items.</Text>}
            {view.items.map((item, index) => {
              const active = item.id === view.selectedId
              const color = item.outcome === 'rejected' ? 'error' : 'warning'
              const type = clip(item.type, Math.max(3, Math.min(columns < 64 ? 10 : 18, columns - 26)))
              const count = columns < 64 ? `${item.pages.length}p` : `${item.pages.length} pages`
              const room = columns - type.length - count.length - item.outcome.length - String(index + 1).length - 13
              return (
                <Box key={`review-row-${index}`} flexDirection="row" gap={1}>
                  <Text color={color} bold={active}>{active ? '▶' : ' '}</Text>
                  <Button key={`review-item-${index}`} label={String(index + 1)} hotkey={rowHotkey(index)} plain
                    onPress={() => selectReview($, curatorBin, item.id)} />
                  <Text color={color} inverse bold={active}> {item.outcome} </Text>
                  <Text color="permission" bold={active}>{type}</Text>
                  <Box minWidth={0} flexGrow={1} flexShrink={1}>
                    <Text bold={active} wrap="truncate-end">{clip(item.summary || item.reason || item.id, room)}</Text>
                  </Box>
                  <Text dimColor>{count}</Text>
                </Box>
              )
            })}
          </Box>
          {selected && (
            <Box key="review-detail" flexDirection="column" borderStyle="round" borderColor={selectedColor} paddingX={1} gap={1}>
              <Box flexDirection="column">
                <Text bold wrap="wrap">{textChunks(selected.summary || selected.reason || selected.id)}</Text>
                <Text dimColor wrap="wrap">{selected.type} · {runDate(selected.run)} · {selected.pages.length} pages</Text>
              </Box>
              <Box key="review-why" flexDirection="column">
                <Text bold>Why</Text>
                <Text color="error" wrap="wrap">{textChunks(selected.reason || 'No reason provided.')}</Text>
                {selected.problems.map(problem => <Text color="warning" wrap="wrap">• {textChunks(problem)}</Text>)}
                {selected.problems.length === 0 && <Text dimColor>No Codex problems reported.</Text>}
              </Box>
              {selected.pages.length > 0 && (
                <Box key="review-pages" flexDirection="column">
                  <Text bold>Pages</Text>
                  {columns < 64
                    ? <Text dimColor wrap="wrap">{textChunks(selected.pages.map(shortPage).join(', '))}</Text>
                    : selected.pages.map(page => <Text dimColor wrap="wrap">{shortPage(page)}</Text>)}
                </Box>
              )}
            </Box>
          )}
          {selected && (
            <Box key="review-diff" flexDirection="column" gap={1}>
              <Box flexDirection="column">
                <Text bold color="permission">Changes</Text>
                {diff.subject && <Text dimColor wrap="truncate-end">{diff.subject}</Text>}
              </Box>
              {view.diffLoading && <Text dimColor>Loading diff…</Text>}
              {view.diffError && <Text color="error">{view.diffError}</Text>}
              {diff.files.map((file, index) => (
                <Box key={`diff-file-${index}`} flexDirection="column">
                  <Box flexDirection="row" gap={1}>
                    <Text color={fileColor(file.status)} inverse> {file.status} </Text>
                    <Box minWidth={0} flexShrink={1}>
                      <Text color={fileColor(file.status)} bold wrap="truncate-end">{file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}</Text>
                    </Box>
                  </Box>
                  {file.code && <Code source={file.code} format="diff" path={file.path} wrap="wrap" />}
                  {file.fallback && <Text dimColor wrap="wrap">{textChunks(file.fallback)}</Text>}
                  {file.omittedHunks > 0 && <Text color="warning">{file.omittedHunks} {file.omittedHunks === 1 ? 'hunk' : 'hunks'} omitted (preview limit).</Text>}
                </Box>
              ))}
              {diff.omittedFiles > 0 && <Text color="warning">{diff.omittedFiles} more files omitted (preview limit).</Text>}
              {diff.truncated && <Text dimColor>Preview shortened at whole hunks; total limit 60 KB.</Text>}
            </Box>
          )}
          {confirmation && !view.operation && (
            <Box key="review-confirmation" flexDirection="column" borderStyle="round" borderColor="warning" paddingX={1} gap={1}>
              <Text color="warning" bold wrap="wrap">{confirmation.action === 'land'
                ? `Land ${confirmation.id}? This cherry-picks onto main.` : `Discard ${confirmation.id}?`}</Text>
              <Box flexDirection="row" gap={1}>
                <Button key={`confirm-${confirmation.action}`} label="Confirm" hotkey="y" variant="primary"
                  onPress={() => confirmReview($, bin, curatorBin, confirmation)} />
                <Button key="cancel-review" label="Cancel" hotkey="n" onPress={() => quiet(() => update($, review, value => ({ ...value, confirmation: null })))} />
              </Box>
            </Box>
          )}
          {view.message && <Text color={outcomeColor(view.message)} bold wrap="wrap">{view.message}</Text>}
          <Box key="review-actions" flexDirection="row" flexWrap="wrap" gap={1}>
            {selected && canAct && selected.kind === 'branch' && <Button key="land" label="Land" variant="primary" hotkey="l" onPress={() => armReview($, selected.id, 'land')} />}
            {selected && canAct && <Button key="discard" label="Discard" hotkey="d" onPress={() => armReview($, selected.id, 'discard')} />}
            <Button key="refresh-review" label="Refresh" hotkey="r" onPress={() => refresh($, bin, curatorBin)} />
            <Button key="close-review" label="Close" role="dismiss" onPress={() => closeReview($)} />
          </Box>
        </Box>
      )
    } catch {
      return next(e)
    }
  })
}
