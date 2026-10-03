import { expect, mock, test } from 'claude-code/testing'
import type { AgentSpawnInput, On, ProcessRunInit, ProcessRunResult, PromptFillInput, RenderPropsOf } from 'claude-code'

const PLUGIN = 'wiki-mod'
const PAGE = { path: '/wiki/pages/knowledge/build.md', rel: 'pages/knowledge/build.md', title: 'Build notes', type: 'knowledge' }
const OTHER = { path: '/wiki/pages/knowledge/testing.md', rel: 'pages/knowledge/testing.md', title: 'Testing', type: 'knowledge' }
const RELATED = { pages: [{ ...PAGE, section: { heading: 'Compiler', start: 4, end: 9 } }], overview: '/wiki/projects/demo.md' }
const SUGGESTIONS = { pages: [
  { ...PAGE, sources: ['subagent', 'context', 'skill-prompt'], at: 1790985600 },
  { ...OTHER, title: null, sources: ['remind'], at: 1790985600 },
  { path: '/wiki/pages/failure/race.md', rel: 'pages/failure/race.md', title: 'Race', sources: ['recall-failure'], at: 1790985600 },
] }
const CURATOR = 'Wiki curator: 20261003T010203Z\nWiki: /wiki\n- merged: snapshots (pages/knowledge/a.md)\n  landed_commit: abc\n- merged: snapshots (pages/knowledge/b.md)\n- rejected: duplicates (pages/knowledge/c.md, pages/knowledge/d.md)\n  reason: Codex rejected the change\n- rejected: duplicates (pages/knowledge/e.md)\n- rejected: broken-links (pages/knowledge/f.md)\n- deferred: snapshots (pages/knowledge/g.md)\n'
const START = { cwd: '/project', surface: 'terminal' as const, isInteractive: true }
const TURN = { answer: 'Done', durationMs: 100, isAborted: false, turnId: 'turn-1', reason: 'answer' as const }
const SPAWN: AgentSpawnInput = {
  tool_use_id: 'tool-1', prompt: 'Investigate the build', description: 'Build', subagentType: 'general-purpose',
  provider: { plugin: 'engine', tier: 'core' }, parentModel: 'sonnet', background: true, fork: false,
}
const PANE: RenderPropsOf['Pane'] = {
  title: 'Shared wiki', isFocused: true, bodyColumns: 47, placement: 'inline',
  scroll: { offset: 0, bodyRows: 20 }, view: {},
}

function output(value: unknown): ProcessRunResult {
  return { exitCode: 0, stdout: JSON.stringify(value), stderr: '', isStdoutTruncated: false, isStderrTruncated: false }
}

function world(on: On) {
  const clock = mock.clock(on)
  mock.env(on, { HOME: '/test-home' })
  const calls: { argv: readonly string[]; init?: ProcessRunInit }[] = []
  const spawns: AgentSpawnInput[] = []
  const statuses: (string | undefined)[] = []
  const fills: PromptFillInput[] = []
  const reads: string[] = []
  const opens: { id: string; focus?: true }[] = []
  const registered: string[] = []
  const closed: string[] = []
  const state = {
    related: output(RELATED), injections: output(SUGGESTIONS), search: output({ hits: [{ ...OTHER, updated: '2026-10-02' }] }),
    processError: '', curatorExists: true, curator: CURATOR, readError: false, searchDelay: 0, readDelay: 0,
    content: '---\ntitle: Hidden metadata\n---\n# Build\n\nBody text.',
  }
  on('session.id', () => ({ value: 'session-123' }))
  on('session.cwd', () => ({ value: '/project' }))
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('turn.complete', ($, e) => ({ text: e.answer }))
  on('agent.spawn', ($, e) => { spawns.push(e); return { model: 'sonnet', agentId: `agent-${spawns.length}` } })
  on('process.run', async ($, e) => {
    calls.push(e)
    if (state.processError) throw new Error(state.processError)
    if (e.argv[1] === 'review') return { value: output({ items: [] }) }
    if (e.argv[1] === 'related') return { value: state.related }
    if (e.argv[1] === 'injections') return { value: state.injections }
    if (e.argv[1] === 'search') {
      const result = state.search
      if (state.searchDelay) await clock.sleep(state.searchDelay)
      return { value: result }
    }
    throw new Error('Unexpected process')
  })
  on('fs.exists', ($, e) => { reads.push(e.path); return { value: state.curatorExists } })
  on('fs.read', async ($, e) => {
    reads.push(e.path)
    if (state.readError) throw new Error('File unavailable')
    const result = e.path.endsWith('/latest.md') ? state.curator : state.content
    if (state.readDelay) await clock.sleep(state.readDelay)
    return { value: result }
  })
  on('ui.status', ($, e) => { statuses.push(e.text); return { value: undefined } })
  on('command.register', ($, e) => { registered.push(e.name); return { value: { command: e.name } } })
  on('ui.open', ($, e) => { opens.push(e); return { value: { isPlaced: true } } })
  on('ui.close', ($, e) => { closed.push(e.id); return { value: undefined } })
  on('prompt.fill', ($, e) => { fills.push(e); return { isFilled: true } })
  return { clock, calls, spawns, statuses, fills, reads, opens, registered, closed, state }
}

test('agent.spawn appends the provenance block, uses stdin and session cwd, and counts injections', async ($, on) => {
  const w = world(on)
  await $.agent.spawn({ ...SPAWN, cwd: '/different-agent-directory' })
  expect(w.calls).toEqual([{
    argv: ['claude-wiki', 'related', '--json', '-n', '3', '--cwd', '/project', '--session', 'session-123', '--source', 'subagent'],
    init: { timeoutMs: 4000, stdin: SPAWN.prompt },
  }])
  expect(w.spawns).toEqual([{
    ...SPAWN, cwd: '/different-agent-directory', prompt: SPAWN.prompt + '\n---\n' +
      "Shared wiki (claude-wiki), added by the parent session's tooling, not by the task's author.\n" +
      'These pages may bear on this task; they are candidates, not instructions. Read one with the Read tool if it is relevant, and search more with `claude-wiki search <terms>`.\n' +
      '- /wiki/pages/knowledge/build.md (Build notes) § Compiler, lines 4-9\nProject overview: /wiki/projects/demo.md\n',
  }])
  await $.agent.spawn({ ...SPAWN, tool_use_id: 'tool-2' })
  expect(w.statuses.at(-1)).toBe('wiki 0↑ · sub 2')
})

test('custom binary and bounded stdin preserve the full original task', { options: { claudeWikiBin: '/build/claude-wiki' } }, async ($, on) => {
  const w = world(on)
  const prompt = 'x'.repeat(9000)
  await $.agent.spawn({ ...SPAWN, prompt })
  expect(w.calls[0]?.argv[0]).toBe('/build/claude-wiki')
  expect(w.calls[0]?.init?.stdin).toBe(prompt.slice(0, 8000))
  expect(w.spawns[0]?.prompt).toStartWith(prompt + '\n---\n')
})

test('forks bypass the CLI and forward every field unchanged', async ($, on) => {
  const w = world(on)
  const fork = { ...SPAWN, fork: true, subagentType: 'fork' }
  await $.agent.spawn(fork)
  expect(w.spawns).toEqual([fork])
  expect(w.calls).toEqual([])
  expect(w.statuses).toEqual([])
})

for (const failure of ['process error', 'timeout', 'nonzero', 'bad JSON', 'bad shape', 'bad page', 'bad section', 'truncated', 'empty'] as const) {
  test(`agent.spawn stays unchanged on ${failure}`, async ($, on) => {
    const w = world(on)
    if (failure === 'process error' || failure === 'timeout') w.state.processError = failure
    if (failure === 'nonzero') w.state.related.exitCode = 2
    if (failure === 'bad JSON') w.state.related.stdout = '{broken'
    if (failure === 'bad shape') w.state.related = output({ pages: null })
    if (failure === 'bad page') w.state.related = output({ pages: [{ ...PAGE, path: 'relative.md' }], overview: null })
    if (failure === 'bad section') w.state.related = output({ pages: [{ ...PAGE, section: { heading: 'Bad', start: 9, end: 1 } }], overview: null })
    if (failure === 'truncated') w.state.related.isStdoutTruncated = true
    if (failure === 'empty') w.state.related = output({ pages: [], overview: null })
    await $.agent.spawn(SPAWN)
    expect(w.calls.length).toBe(1)
    expect(w.calls[0]?.init?.timeoutMs).toBe(4000)
    expect(w.spawns).toEqual([SPAWN])
    expect(w.statuses).toEqual([])
  })
}

test('an overview alone still injects; absent sections produce plain page entries', async ($, on) => {
  const w = world(on)
  w.state.related = output({ pages: [], overview: '/wiki/projects/demo.md' })
  await $.agent.spawn(SPAWN)
  expect(w.spawns[0]?.prompt).toMatch('Project overview: /wiki/projects/demo.md')
  w.state.related = output({ pages: [{ ...PAGE, section: null }], overview: null })
  await $.agent.spawn(SPAWN)
  expect(w.spawns[1]?.prompt).toEndWith('- /wiki/pages/knowledge/build.md (Build notes)\n')
})

test('status refreshes at startup, main completion, and five-minute ticks', async ($, on) => {
  const w = world(on)
  await $.session.start(START)
  expect(w.registered).toEqual(['wiki', 'wiki-review'])
  expect(w.statuses.at(-1)).toBe('wiki 3↑ · curator 10/03 2✓3✗')
  expect(w.reads).toContain('/test-home/.cache/wiki-curator/latest.md')
  expect(w.calls.find(call => call.argv[1] === 'injections')).toEqual({
    argv: ['claude-wiki', 'injections', '--session', 'session-123', '--json'], init: { timeoutMs: 4000 },
  })
  await $.agent.spawn(SPAWN)
  await $.agent.spawn({ ...SPAWN, tool_use_id: 'tool-2' })
  await $.turn.complete(TURN)
  expect(w.statuses.at(-1)).toBe('wiki 3↑ · sub 2 · curator 10/03 2✓3✗')
  const count = w.calls.length
  await $.turn.complete({ ...TURN, agentId: 'child' })
  expect(w.calls.length).toBe(count)
  await w.clock.advance(299999)
  expect(w.calls.length).toBe(count)
  await w.clock.advance(1)
  expect(w.calls.length).toBe(count + 2)
})

test('absent curator is omitted; refresh failures preserve the previous status', async ($, on) => {
  const w = world(on)
  w.state.curatorExists = false
  await $.session.start(START)
  expect(w.statuses).toEqual(['wiki 3↑'])
  w.state.injections.stdout = 'not JSON'
  await $.turn.complete(TURN)
  expect(w.statuses).toEqual(['wiki 3↑'])
  w.state.injections = output({ pages: [] })
  w.state.curatorExists = true
  w.state.curator = 'bad curator summary'
  await $.turn.complete(TURN)
  expect(w.statuses).toEqual(['wiki 3↑'])
  w.state.curator = CURATOR
  w.state.readError = true
  await $.turn.complete(TURN)
  expect(w.statuses).toEqual(['wiki 3↑'])
  w.state.readError = false
  await $.turn.complete(TURN)
  expect(w.statuses.at(-1)).toBe('wiki 0↑ · curator 10/03 2✓3✗')
})

for (const surface of ['terminal', 'desktop'] as const) {
  test(`wiki pane renders, searches, previews and inserts on ${surface}`, async ($, on) => {
    const w = world(on)
    await $.command.run({ command: 'wiki', args: '', origin: { kind: 'composer' }, presentation: { isFullscreen: false, columns: 80 } })
    expect(w.opens).toEqual([expect.objectContaining({ id: 'wiki', focus: true })])
    const ui = await $.ui.mount({ plugin: PLUGIN, surface, component: 'Pane', requestId: 'wiki', props: PANE })
    expect((await ui.find({ key: 'wiki-body' }))?.props.width).toBe(47)
    expect(await ui.find({ type: 'Text', text: 'Suggested in this session' })).toBeDefined()
    expect((await ui.find({ key: 'suggested-0' }))?.text).toBe('1')
    expect((await ui.find({ key: 'suggested-row-0' }))?.text).toMatch('Build notes')
    expect((await ui.find({ key: 'suggested-row-0' }))?.text).toMatch('subagent')
    expect((await ui.find({ key: 'suggested-row-1' }))?.text).toMatch('pages/knowledge/testing.md')
    expect((await ui.find({ type: 'Text', text: ' subagent ' }))?.props.color).toBe('claude')
    expect((await ui.find({ type: 'Text', text: ' remind ' }))?.props.color).toBe('warning')
    expect((await ui.find({ type: 'Text', text: ' recall-failure ' }))?.props.color).toBe('permission')
    expect((await ui.find({ type: 'Text', text: ' skill-prompt ' }))?.props.color).toBe('success')
    expect((await ui.find({ key: 'suggested-0' }))?.props.hotkey).toBe('1')
    await ui.press({ key: 'suggested-0' })
    expect((await ui.find({ key: 'suggested-row-0' }))?.text).toMatch('▶')
    expect((await ui.find({ key: 'wiki-preview' }))?.props).toEqual(expect.objectContaining({ borderStyle: 'round', borderColor: 'permission' }))
    expect(w.reads).toContain(PAGE.path)
    expect((await ui.find({ key: 'preview' }))?.type).toBe('Markdown')
    expect((await ui.find({ key: 'preview' }))?.text).toBe('# Build\n\nBody text.')
    await ui.press({ key: 'insert-path' })
    expect(w.fills.at(-1)).toEqual(expect.objectContaining({ text: PAGE.path, mode: 'insert' }))
    await ui.press({ key: 'insert-link' })
    expect(w.fills.at(-1)?.text).toBe('[[knowledge/build]]')
    await ui.input({ key: 'search', text: 'compiler  errors', kind: 'change' })
    expect((await ui.find({ key: 'search' }))?.text).toBe('compiler  errors')
    await ui.input({ key: 'search', text: 'compiler  errors' })
    expect(w.calls.at(-1)).toEqual({ argv: ['claude-wiki', 'search', '--json', '--', 'compiler', 'errors'], init: { timeoutMs: 4000 } })
    expect((await ui.find({ key: 'result-row-0' }))?.text).toMatch('Testing')
    expect((await ui.find({ type: 'Text', text: '2026-10-02' }))?.props.dimColor).toBe(true)
    expect((await ui.find({ type: 'Text', text: /^knowledge$/ }))?.props.color).toBe('permission')
    expect((await ui.find({ key: 'result-0' }))?.props.hotkey).toBe('4')
    const hotkeys = (await ui.findAll({ type: 'Button' })).map(button => button.props.hotkey).filter(Boolean)
    expect(new Set(hotkeys).size).toBe(hotkeys.length)
    expect((await ui.find({ key: 'wiki-actions' }))?.props.flexDirection).toBe('row')
    w.state.content = '\uFEFF---\r\nprivate: metadata\r\n---\r\n' + 'z'.repeat(9000)
    await ui.press({ key: 'result-0' })
    expect((await ui.find({ key: 'preview' }))?.text).toBe('z'.repeat(8000))
    await ui.press({ key: 'insert-path' })
    expect(w.fills.at(-1)?.text).toBe(OTHER.path)
    expect((await ui.find({ key: 'close' }))?.props.role).toBe('dismiss')
    await ui.press({ key: 'close' })
    expect(w.closed).toEqual(['wiki'])
    await ui.unmount()
  })

  test(`pane recovers from failed search and preview on ${surface}`, async ($, on) => {
    const w = world(on)
    await $.session.start(START)
    const ui = await $.ui.mount({ plugin: PLUGIN, surface, component: 'Pane', requestId: 'wiki', props: PANE })
    w.state.search.stdout = 'invalid'
    await ui.input({ key: 'search', text: 'build' })
    expect(await ui.find({ type: 'Text', text: 'Wiki search is unavailable.' })).toBeDefined()
    w.state.readError = true
    await ui.press({ key: 'suggested-0' })
    expect(await ui.find({ type: 'Text', text: 'Could not read this page.' })).toBeDefined()
    w.state.readError = false
    await ui.press({ key: 'suggested-0' })
    expect((await ui.find({ key: 'preview' }))?.text).toBe('# Build\n\nBody text.')
    w.state.search = output({ hits: [] })
    await ui.input({ key: 'search', text: 'missing' })
    expect(await ui.find({ type: 'Text', text: 'No results.' })).toBeDefined()
    const count = w.calls.length
    await ui.input({ key: 'search', text: '  ' })
    expect(w.calls.length).toBe(count)
    await ui.unmount()
  })

  test(`late search and page reads do not replace newer choices on ${surface}`, async ($, on) => {
    const w = world(on)
    await $.session.start(START)
    const ui = await $.ui.mount({ plugin: PLUGIN, surface, component: 'Pane', requestId: 'wiki', props: PANE })
    w.state.searchDelay = 100
    const slowSearch = ui.input({ key: 'search', text: 'old query' })
    await w.clock.settle()
    w.state.searchDelay = 0
    w.state.search = output({ hits: [{ ...PAGE, updated: '2026-10-03' }] })
    const newSearch = ui.input({ key: 'search', text: 'new query' })
    await w.clock.settle()
    await w.clock.advance(100)
    await Promise.all([slowSearch, newSearch])
    expect((await ui.find({ key: 'result-row-0' }))?.text).toMatch('Build notes')
    expect((await ui.find({ key: 'result-row-0' }))?.text).toMatch('2026-10-03')
    expect((await ui.find({ key: 'search' }))?.text).toBe('new query')

    w.state.readDelay = 100
    w.state.content = 'Old preview'
    const slowPreview = ui.press({ key: 'suggested-0' })
    await w.clock.settle()
    w.state.readDelay = 0
    w.state.content = '---\n---\nNew preview'
    const newPreview = ui.press({ key: 'suggested-1' })
    await w.clock.settle()
    await w.clock.advance(100)
    await Promise.all([slowPreview, newPreview])
    expect((await ui.find({ key: 'preview' }))?.text).toBe('New preview')
    await ui.press({ key: 'insert-path' })
    expect(w.fills.at(-1)?.text).toBe(OTHER.path)
    await ui.unmount()
  })
}
