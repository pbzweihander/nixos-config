import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'
import type { WikiPane, WikiReview, WikiReviewAction, WikiReviewConfirmation, WikiStats } from '../types'
import { curatorSummary, injectionBlock, parseHits, parseRelated, parseReview, parseSuggestions, previewText, reviewDiff, reviewOutcome, skillBlock, statusText, textChunks, wikiLink } from './wiki'

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
          ...(same ? {} : { selectedId: null, diff: '', diffError: '', diffLoading: false, diffTruncated: false, selectionRequest: value.selectionRequest + 1 }),
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
      diff: '', diffLoading: true, diffTruncated: false, diffError: '', message: '',
    })
    if (selected.operation || selected.selectedId !== id) return
    const { selectionRequest } = selected
    try {
      const result = await $.process.run([curatorBin, 'show', id], { timeoutMs: 4000 })
      if (result.exitCode !== 0 || result.isStdoutTruncated) throw new Error('Diff unavailable')
      const preview = reviewDiff(result.stdout)
      await update($, review, value => value.selectionRequest === selectionRequest
        ? { ...value, diff: preview.text, diffTruncated: preview.truncated, diffLoading: false } : value)
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
      return (
        <Box key="wiki-body" flexDirection="column" width={Math.max(1, e.props.bodyColumns)}>
          <Text bold>Suggested in this session</Text>
          {session.suggested.length === 0 && <Text dimColor>No suggestions yet.</Text>}
          {session.suggested.map((page, index) => (
            <Button key={`suggested-${index}`} label={`${page.title} (${page.sources.join(', ')})`}
              onPress={() => selectPage($, page.path)} />
          ))}
          <Text bold>Search</Text>
          <Input key="search" label="Search wiki" placeholder="Search terms" value={view.query} submitLabel="search" autoFocus
            onInput={query => quiet(() => update($, pane, value => ({ ...value, query })))}
            onSubmit={query => search($, bin, query)} />
          {view.searching && <Text dimColor>Searching…</Text>}
          {view.searchError && <Text>{view.searchError}</Text>}
          {view.searched && !view.searching && !view.searchError && view.hits.length === 0 && <Text dimColor>No results.</Text>}
          {view.hits.map((page, index) => (
            <Button key={`result-${index}`} label={`${page.title} · ${page.type} · ${page.updated || 'unknown date'}`}
              onPress={() => selectPage($, page.path)} />
          ))}
          <Text bold>Preview</Text>
          {view.selected === null && <Text dimColor>Select a page to preview it.</Text>}
          {view.selected !== null && <Text>{view.selected.title}</Text>}
          {view.loading && <Text dimColor>Loading…</Text>}
          {view.previewError && <Text>{view.previewError}</Text>}
          {view.preview && <Markdown key="preview" text={view.preview} />}
          {view.selected !== null && (
            <Box flexDirection="column">
              <Button key="insert-path" label="Insert path" onPress={() => insert($, 'path')} />
              <Button key="insert-link" label="Insert link" onPress={() => insert($, 'link')} />
            </Box>
          )}
          {view.actionError && <Text>{view.actionError}</Text>}
          <Button key="review-curator" label="Review curator" onPress={() => openReview($, bin, curatorBin)} />
          <Button key="close" label="Close" role="dismiss" onPress={() => quiet(() => $.ui.close({ id: PANE }))} />
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
      const selected = view.items.find(item => item.id === view.selectedId)
      const confirmation = view.confirmation
      // Code has a 10,000-character limit, and a cut hunk may not parse.
      const useCode = view.diff.length <= 10000 && !view.diffTruncated && /^@@ -\d/m.test(view.diff)
      return (
        <Box key="review-body" flexDirection="column" width={Math.max(1, e.props.bodyColumns)}>
          <Text bold>Curator review</Text>
          {view.listLoading && <Text dimColor>Refreshing…</Text>}
          {view.listError && <Text>{view.listError}</Text>}
          {!view.listLoading && !view.listError && view.items.length === 0 && <Text dimColor>No pending curator items.</Text>}
          {view.items.map((item, index) => (
            <Button key={`review-item-${index}`} label={`${item.type} · ${item.outcome} · ${(item.summary || item.reason || item.id).slice(0, 300)} · ${item.pages.length} pages`}
              onPress={() => selectReview($, curatorBin, item.id)} />
          ))}
          {selected && (
            <Box flexDirection="column">
              <Text bold>{selected.id}</Text>
              <Text>Reason: {textChunks(selected.reason || 'No reason provided.')}</Text>
              <Text bold>Codex problems</Text>
              {selected.problems.length === 0 && <Text dimColor>No Codex problems reported.</Text>}
              {selected.problems.map(problem => <Text>{textChunks(problem)}</Text>)}
              {view.diffLoading && <Text dimColor>Loading diff…</Text>}
              {view.diffError && <Text>{view.diffError}</Text>}
              {view.diff && (useCode ? <Code source={view.diff} format="diff" /> : <Text>{textChunks(view.diff)}</Text>)}
              {view.diffTruncated && <Text dimColor>Diff preview truncated at 60 KB.</Text>}
              {!view.operation && !view.listLoading && !view.listError && !view.diffLoading && !confirmation && (
                <Box flexDirection="column">
                  {selected.kind === 'branch' && <Button key="land" label="Land" onPress={() => armReview($, selected.id, 'land')} />}
                  <Button key="discard" label="Discard" onPress={() => armReview($, selected.id, 'discard')} />
                </Box>
              )}
              {confirmation && !view.operation && (
                <Box flexDirection="column">
                  <Text>{confirmation.action === 'land' ? 'Land' : 'Discard'} {confirmation.id}?</Text>
                  <Button key={`confirm-${confirmation.action}`} label={`Confirm ${confirmation.action === 'land' ? 'Land' : 'Discard'}`}
                    onPress={() => confirmReview($, bin, curatorBin, confirmation)} />
                  <Button key="cancel-review" label="Cancel" onPress={() => quiet(() => update($, review, value => ({ ...value, confirmation: null })))} />
                </Box>
              )}
            </Box>
          )}
          {view.message && <Text>{view.message}</Text>}
          <Button key="refresh-review" label="Refresh" onPress={() => refresh($, bin, curatorBin)} />
          <Button key="close-review" label="Close" role="dismiss" onPress={() => closeReview($)} />
        </Box>
      )
    } catch {
      return next(e)
    }
  })
}
