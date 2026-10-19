import { expect, mock, test } from 'claude-code/testing'
import type { On, ProcessRunInit, ProcessRunResult, RenderPropsOf, SkillPromptInput } from 'claude-code'

const BRANCH = {
  id: 'curator/rejected-1', kind: 'branch', branch: 'curator/rejected-1',
  run: '20261003T010203Z', task: 'deduplicate', type: 'duplicates', pages: ['pages/knowledge/a.md', 'pages/knowledge/b.md'],
  outcome: 'rejected', reason: 'References need review', codex_problems: ['A backlink would break'], summary: 'Merge duplicate build notes', commit: 'abc123',
}
const DEFERRED = {
  id: 'curator/deferred/20261003T010203Z-2', kind: 'branch', branch: 'curator/deferred/20261003T010203Z-2',
  run: null, task: null, type: 'snapshots', pages: null, outcome: 'deferred', reason: 'Needs approval',
  codex_problems: null, summary: null, commit: null,
}
const DIFF = 'diff --git a/pages/knowledge/a.md b/pages/knowledge/a.md\n--- a/pages/knowledge/a.md\n+++ b/pages/knowledge/a.md\n@@ -1 +1 @@\n-old\n+new\n'
const PAGE = { path: '/wiki/pages/knowledge/build.md', rel: 'pages/knowledge/build.md', title: 'Build notes', type: 'knowledge' }
const START = { cwd: '/project', surface: 'terminal' as const, isInteractive: true }
const TURN = { answer: 'Done', durationMs: 1, isAborted: false, turnId: 'turn-1', reason: 'answer' as const }
const PANE: RenderPropsOf['Pane'] = {
  title: 'Curator review', isFocused: true, bodyColumns: 51, placement: 'inline',
  scroll: { offset: 0, bodyRows: 24 }, view: {},
}
const COMMAND = { command: 'wiki-review', args: '', origin: { kind: 'composer' as const }, presentation: { isFullscreen: false, columns: 80 } }

function output(value: unknown, exitCode = 0): ProcessRunResult {
  return { exitCode, stdout: JSON.stringify(value), stderr: '', isStdoutTruncated: false, isStderrTruncated: false }
}

function world(on: On) {
  const clock = mock.clock(on)
  mock.env(on, { HOME: '/test-home' })
  const calls: { argv: readonly string[]; init?: ProcessRunInit }[] = []
  const statuses: (string | undefined)[] = []
  const skills: SkillPromptInput[] = []
  const opens: { id: string; focus?: true }[] = []
  const state = {
    review: output({ items: [BRANCH, DEFERRED] }),
    show: { ...output(null), stdout: DIFF },
    skill: output({ pages: [PAGE] }),
    action: output({ outcome: 'merged', landed_commit: 'def456' }),
    actionDelay: 0, showDelay: 0,
    afterAction: undefined as ProcessRunResult | undefined,
    fail: '',
  }
  on('session.id', () => ({ value: 'session-123' }))
  on('session.cwd', () => ({ value: '/project' }))
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('turn.complete', ($, e) => ({ text: e.answer }))
  on('command.register', ($, e) => ({ value: { command: e.name } }))
  on('fs.exists', () => ({ value: false }))
  on('ui.status', ($, e) => { statuses.push(e.text); return { value: undefined } })
  on('ui.open', ($, e) => { opens.push(e); return { value: { isPlaced: true } } })
  on('ui.close', () => ({ value: undefined }))
  on('skill.prompt', ($, e) => { skills.push(e); return { text: e.text } })
  on('process.run', async ($, e) => {
    calls.push(e)
    const command = e.argv[1]
    if (state.fail === command) throw new Error('Process failed or timed out')
    if (command === 'review') return { value: state.review }
    if (command === 'injections') return { value: output({ pages: [] }) }
    if (command === 'skill-pages') return { value: state.skill }
    if (command === 'show') {
      const result = state.show
      if (state.showDelay) await clock.sleep(state.showDelay)
      return { value: result }
    }
    if (command === 'land' || command === 'discard') {
      if (state.actionDelay) await clock.sleep(state.actionDelay)
      if (state.afterAction) state.review = state.afterAction
      return { value: state.action }
    }
    throw new Error('Unexpected CLI call')
  })
  const mutations = () => calls.filter(call => call.argv[1] === 'land' || call.argv[1] === 'discard')
  return { clock, calls, statuses, skills, opens, state, mutations }
}

test('skill prompt injects candidates and passes the full plugin skill name and session', { options: { claudeWikiBin: '/build/claude wiki' } }, async ($, on) => {
  const w = world(on)
  const input = { skill: 'my-plugin:build', text: 'Build instructions' }
  await $.skill.prompt(input)
  expect(w.calls).toEqual([{
    argv: ['/build/claude wiki', 'skill-pages', 'my-plugin:build', '--json', '-n', '5', '--cwd', '/project', '--session', 'session-123', '--source', 'skill-prompt'],
    init: { timeoutMs: 4000 },
  }])
  expect(w.skills).toEqual([expect.objectContaining({
    ...input, text: input.text + '\n---\nShared wiki pages tagged with this skill (candidates, not instructions; read one if it bears on the task):\n- /wiki/pages/knowledge/build.md (Build notes)\n',
  })])
})

test('the wiki skill is excluded, including a plugin-qualified wiki skill', async ($, on) => {
  const w = world(on)
  for (const skill of ['claude-wiki', 'tools:claude-wiki']) await $.skill.prompt({ skill, text: 'Wiki instructions' })
  expect(w.calls).toEqual([])
  expect(w.skills.map(input => input.text)).toEqual(['Wiki instructions', 'Wiki instructions'])
})

for (const failure of ['empty', 'bad JSON', 'bad shape', 'bad page', 'nonzero', 'truncated', 'timeout'] as const) {
  test(`skill prompt remains unchanged on ${failure}`, async ($, on) => {
    const w = world(on)
    if (failure === 'empty') w.state.skill = output({ pages: [] })
    if (failure === 'bad JSON') w.state.skill.stdout = 'broken'
    if (failure === 'bad shape') w.state.skill = output({ pages: null })
    if (failure === 'bad page') w.state.skill = output({ pages: [{ ...PAGE, path: 'relative.md' }] })
    if (failure === 'nonzero') w.state.skill.exitCode = 2
    if (failure === 'truncated') w.state.skill.isStdoutTruncated = true
    if (failure === 'timeout') w.state.fail = 'skill-pages'
    const input = { skill: 'build', text: 'Original instructions' }
    await $.skill.prompt(input)
    expect(w.skills).toEqual([expect.objectContaining(input)])
    expect(w.calls.length).toBe(1)
  })
}

for (const surface of ['terminal', 'desktop'] as const) {
  test(`numbered selection and action hotkeys keep the confirmation gate on ${surface}`, async ($, on) => {
    const w = world(on)
    w.state.review = output({ items: Array.from({ length: 10 }, (_, index) => ({ ...BRANCH, id: `branch-${index}`, branch: `branch-${index}` })) })
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki-review', props: PANE })
    const pressHotkey = async (hotkey: string) => {
      const buttons = (await ui.findAll({ type: 'Button' })).filter(button => button.props.hotkey === hotkey)
      expect(buttons.length).toBe(1)
      await ui.press({ key: buttons[0]?.key ?? '' })
    }
    expect((await ui.find({ key: 'review-item-8' }))?.props.hotkey).toBe('9')
    expect((await ui.find({ key: 'review-item-9' }))?.props.hotkey).toBeUndefined()
    await pressHotkey('1')
    expect((await ui.find({ key: 'review-actions' }))?.props.flexDirection).toBe('row')
    expect((await ui.find({ key: 'land' }))?.props).toEqual(expect.objectContaining({ hotkey: 'l', variant: 'primary' }))
    expect((await ui.find({ key: 'discard' }))?.props.hotkey).toBe('d')
    expect((await ui.find({ key: 'refresh-review' }))?.props.hotkey).toBe('r')
    expect((await ui.find({ key: 'close-review' }))?.props.role).toBe('dismiss')
    await pressHotkey('l')
    expect(w.mutations()).toEqual([])
    await pressHotkey('n')
    expect(w.mutations()).toEqual([])
    await pressHotkey('d')
    expect(w.mutations()).toEqual([])
    expect((await ui.find({ key: 'review-confirmation' }))?.text).toMatch('Discard branch-0?')
    w.state.action = output({ outcome: 'discarded' })
    await pressHotkey('y')
    expect(w.mutations().map(call => call.argv.slice(1))).toEqual([['discard', 'branch-0', '--json']])
    await ui.unmount()
  })

  test(`large git show keeps individual files colored on ${surface}`, async ($, on) => {
    const w = world(on)
    const hunk = '@@ -1 +1 @@\n-' + 'a'.repeat(3000) + '\n+' + 'b'.repeat(3000) + '\n'
    w.state.show.stdout = 'commit abc123\nAuthor: Person\n\n    Snapshot update\n\n snapshots.md | many changes\n\n' +
      'diff --git a/snapshots.md b/snapshots.md\n--- a/snapshots.md\n+++ b/snapshots.md\n' + hunk + hunk.replace('@@ -1 +1 @@', '@@ -20 +20 @@') +
      'diff --git a/old.md b/new.md\nrename from old.md\nrename to new.md\n--- a/old.md\n+++ b/new.md\n@@ -1 +1 @@\n-old\n+new\n' +
      'diff --git a/added.md b/added.md\nnew file mode 100644\n--- /dev/null\n+++ b/added.md\n@@ -0,0 +1 @@\n+added\n' +
      'diff --git a/deleted.md b/deleted.md\ndeleted file mode 100644\n--- a/deleted.md\n+++ /dev/null\n@@ -1 +0,0 @@\n-deleted\n'
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki-review', props: PANE })
    await ui.press({ key: 'review-item-0' })
    const code = await ui.findAll({ type: 'Code' })
    expect(code.length).toBe(4)
    expect(code.map(element => element.props.path)).toEqual(['snapshots.md', 'new.md', 'added.md', 'deleted.md'])
    for (const element of code) {
      expect(element.props.format).toBe('diff')
      expect(String(element.props.source).length).toBeLessThanOrEqual(10000)
      expect(String(element.props.source)).toStartWith('@@ ')
    }
    expect((await ui.find({ type: 'Text', text: /^Snapshot update$/ }))?.props.dimColor).toBe(true)
    expect(await ui.find({ type: 'Text', text: 'Author: Person' })).toBeUndefined()
    expect(await ui.find({ type: 'Text', text: '1 hunk omitted' })).toBeDefined()
    for (const [status, color] of [['M', 'warning'], ['R', 'permission'], ['A', 'success'], ['D', 'error']] as const) {
      expect((await ui.find({ type: 'Text', text: new RegExp(`^ ${status} $`) }))?.props.color).toBe(color)
    }
    await ui.unmount()
  })

  test(`curator command and wiki button open the review pane on ${surface}`, { options: { wikiCuratorBin: '/build/wiki curator' } }, async ($, on) => {
    const w = world(on)
    await $.session.start(START)
    expect(w.statuses.at(-1)).toBe('wiki 0↑ · review 2')
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki-review', props: PANE })
    expect((await ui.find({ key: 'review-body' }))?.props.width).toBe(51)
    expect((await ui.find({ key: 'review-item-0' }))?.text).toBe('1')
    expect((await ui.find({ key: 'review-item-0' }))?.props).toEqual(expect.objectContaining({ plain: true, hotkey: '1' }))
    expect((await ui.find({ key: 'review-row-0' }))?.text).toMatch('rejected')
    expect((await ui.find({ key: 'review-row-1' }))?.text).toMatch('deferred')
    expect((await ui.find({ type: 'Text', text: '1 rejected' }))?.props).toEqual(expect.objectContaining({ color: 'error', inverse: true }))
    expect((await ui.find({ type: 'Text', text: '1 deferred' }))?.props).toEqual(expect.objectContaining({ color: 'warning', inverse: true }))
    await ui.press({ key: 'review-item-0' })
    expect((await ui.find({ key: 'review-row-0' }))?.text).toMatch('▶')
    expect((await ui.find({ key: 'review-detail' }))?.props).toEqual(expect.objectContaining({ borderStyle: 'round', borderColor: 'error', paddingX: 1 }))
    expect((await ui.find({ type: 'Text', text: 'Merge duplicate build notes' }))?.props.bold).toBe(true)
    expect((await ui.find({ type: 'Text', text: '10-03 01:02' }))?.props.dimColor).toBe(true)
    expect((await ui.find({ type: 'Text', text: 'References need review' }))?.props.color).toBe('error')
    expect((await ui.find({ type: 'Text', text: '• A backlink would break' }))?.props.color).toBe('warning')
    expect((await ui.find({ key: 'review-pages' }))?.text).toMatch('knowledge/a, knowledge/b')
    expect((await ui.find({ type: 'Code' }))?.props.format).toBe('diff')
    expect((await ui.find({ type: 'Code' }))?.props.source).toBe('@@ -1 +1 @@\n-old\n+new\n')
    expect((await ui.find({ type: 'Code' }))?.props.path).toBe('pages/knowledge/a.md')
    expect(w.calls.at(-1)).toEqual({ argv: ['/build/wiki curator', 'show', BRANCH.id], init: { timeoutMs: 4000 } })
    expect(w.mutations()).toEqual([])
    const wiki = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki', props: PANE })
    await wiki.press({ key: 'review-curator' })
    expect(w.opens).toEqual([
      expect.objectContaining({ id: 'wiki-review', focus: true }), expect.objectContaining({ id: 'wiki-review', focus: true }),
    ])
    await wiki.unmount()
    await ui.unmount()
  })

  test(`Land requires confirmation, cannot run twice, and refreshes outcomes on ${surface}`, async ($, on) => {
    const w = world(on)
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki-review', props: PANE })
    await ui.press({ key: 'review-item-0' })
    await ui.press({ key: 'land' })
    expect(w.mutations()).toEqual([])
    expect((await ui.find({ key: 'confirm-land' }))?.text).toBe('Confirm')
    expect((await ui.find({ key: 'confirm-land' }))?.props).toEqual(expect.objectContaining({ hotkey: 'y', variant: 'primary' }))
    expect((await ui.find({ key: 'cancel-review' }))?.props.hotkey).toBe('n')
    expect((await ui.find({ key: 'review-confirmation' }))?.props.borderColor).toBe('warning')
    expect(await ui.find({ type: 'Text', text: 'This cherry-picks onto main.' })).toBeDefined()
    await ui.press({ key: 'cancel-review' })
    expect(await ui.find({ key: 'confirm-land' })).toBeUndefined()
    expect(w.mutations()).toEqual([])
    await ui.press({ key: 'land' })
    w.state.actionDelay = 130000
    w.state.afterAction = output({ items: [DEFERRED] })
    const first = ui.press({ key: 'confirm-land' })
    const duplicate = ui.press({ key: 'confirm-land' })
    const settled = Promise.allSettled([first, duplicate])
    await w.clock.settle()
    expect(w.mutations().length).toBe(1)
    await w.clock.advance(130000)
    await settled
    expect(w.mutations()).toEqual([{ argv: ['wiki-curator', 'land', BRANCH.branch, '--json'], init: { timeoutMs: 150000 } }])
    expect(await ui.find({ type: 'Text', text: 'Land: merged (def456)' })).toBeDefined()
    expect((await ui.find({ type: 'Text', text: 'Land: merged (def456)' }))?.props.color).toBe('success')
    expect(w.statuses.at(-1)).toBe('wiki 0↑ · review 1')
    expect(await ui.find({ key: 'confirm-land' })).toBeUndefined()
    expect(w.calls.slice(-2).map(call => call.argv[1])).toEqual(['review', 'injections'])
    await ui.unmount()
  })

  test(`deferred branches can be landed or discarded, with confirmation, on ${surface}`, async ($, on) => {
    const w = world(on)
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki-review', props: PANE })
    await ui.press({ key: 'review-item-1' })
    expect(await ui.find({ key: 'land' })).toBeDefined()
    await ui.press({ key: 'discard' })
    expect(w.mutations()).toEqual([])
    w.state.action = output({ outcome: 'deferred', reason: 'A curator run is in progress' }, 3)
    await ui.press({ key: 'confirm-discard' })
    expect(w.mutations()[0]).toEqual({ argv: ['wiki-curator', 'discard', DEFERRED.id, '--json'], init: { timeoutMs: 4000 } })
    expect(await ui.find({ type: 'Text', text: 'Discard: deferred — A curator run is in progress' })).toBeDefined()
    expect((await ui.find({ type: 'Text', text: 'Discard: deferred' }))?.props.color).toBe('warning')
    expect(w.statuses.at(-1)).toBe('wiki 0↑ · review 2')
    expect(await ui.find({ key: 'confirm-discard' })).toBeUndefined()
    await ui.press({ key: 'discard' })
    w.state.action = output({ outcome: 'discarded' })
    w.state.afterAction = output({ items: [] })
    await ui.press({ key: 'confirm-discard' })
    expect(await ui.find({ type: 'Text', text: 'Discard: discarded' })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: 'No pending curator items.' })).toBeDefined()
    expect(w.statuses.at(-1)).toBe('wiki 0↑')
    await ui.unmount()
  })

  test(`legacy diff items are invalid on ${surface}`, async ($, on) => {
    const w = world(on)
    w.state.review = output({ items: [{ ...BRANCH, kind: 'diff', id: 'diff:20261003T010203Z-2' }] })
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki-review', props: PANE })
    expect(await ui.find({ type: 'Text', text: 'Curator review is unavailable.' })).toBeDefined()
    expect(await ui.find({ key: 'review-item-0' })).toBeUndefined()
    expect(await ui.find({ key: 'land' })).toBeUndefined()
    expect(await ui.find({ key: 'discard' })).toBeUndefined()
    expect(w.mutations()).toEqual([])
    await ui.unmount()
  })

  test(`selection, item changes, close, and reload revoke confirmations on ${surface}`, async ($, on) => {
    const w = world(on)
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki-review', props: PANE })
    await ui.press({ key: 'review-item-0' })
    await ui.press({ key: 'land' })
    await ui.press({ key: 'review-item-1' })
    expect(await ui.find({ key: 'confirm-land' })).toBeUndefined()
    await ui.press({ key: 'discard' })
    await ui.press({ key: 'close-review' })
    expect(await ui.find({ key: 'confirm-discard' })).toBeUndefined()
    await ui.press({ key: 'review-item-0' })
    await ui.press({ key: 'land' })
    w.state.review = output({ items: [{ ...BRANCH, commit: 'changed' }, DEFERRED] })
    await ui.press({ key: 'refresh-review' })
    expect(await ui.find({ key: 'confirm-land' })).toBeUndefined()
    await ui.press({ key: 'review-item-0' })
    await ui.press({ key: 'land' })
    await $.session.start(START)
    expect(await ui.find({ key: 'confirm-land' })).toBeUndefined()
    expect(w.mutations()).toEqual([])
    await ui.unmount()
  })

  test(`diff preview caps bytes, falls back to Text, and ignores stale reads on ${surface}`, async ($, on) => {
    const w = world(on)
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki-review', props: PANE })
    w.state.show = { ...output(null), stdout: DIFF + '가'.repeat(30000) }
    await ui.press({ key: 'review-item-0' })
    expect(await ui.find({ type: 'Code' })).toBeUndefined()
    const preview = await ui.find({ type: 'Text', text: 'diff --git' })
    expect(new TextEncoder().encode(preview?.text).length).toBeLessThanOrEqual(60000)
    expect(await ui.find({ type: 'Text', text: 'total limit 60 KB.' })).toBeDefined()
    w.state.show = { ...output(null), stdout: 'old diff' }
    w.state.showDelay = 100
    const old = ui.press({ key: 'review-item-0' })
    await w.clock.settle()
    w.state.showDelay = 0
    w.state.show = { ...output(null), stdout: 'new diff' }
    const recent = ui.press({ key: 'review-item-1' })
    await w.clock.settle()
    await w.clock.advance(100)
    await Promise.all([old, recent])
    expect(await ui.find({ type: 'Text', text: 'new diff' })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: 'old diff' })).toBeUndefined()
    await ui.unmount()
  })

  test(`review and diff failures retain the list and allow retry on ${surface}`, async ($, on) => {
    const w = world(on)
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface, component: 'Pane', requestId: 'wiki-review', props: PANE })
    w.state.review.stdout = 'not JSON'
    await ui.press({ key: 'refresh-review' })
    expect(await ui.find({ key: 'review-item-0' })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: 'Curator review is unavailable.' })).toBeDefined()
    expect(w.statuses.at(-1)).toBe('wiki 0↑ · review 2')
    w.state.review = output({ items: [BRANCH, DEFERRED] })
    await ui.press({ key: 'refresh-review' })
    w.state.fail = 'show'
    await ui.press({ key: 'review-item-0' })
    expect(await ui.find({ type: 'Text', text: 'Could not load the diff.' })).toBeDefined()
    w.state.fail = ''
    await ui.press({ key: 'review-item-0' })
    expect(await ui.find({ type: 'Code' })).toBeDefined()
    expect(w.mutations()).toEqual([])
    await ui.unmount()
  })
}

for (const scenario of ['deferred', 'invalid', 'bad JSON', 'timeout'] as const) {
  test(`confirmed land reports ${scenario} and refreshes without retrying`, async ($, on) => {
    const w = world(on)
    await $.command.run(COMMAND)
    const ui = await $.ui.mount({ plugin: 'wiki-mod', surface: 'terminal', component: 'Pane', requestId: 'wiki-review', props: PANE })
    await ui.press({ key: 'review-item-0' })
    await ui.press({ key: 'land' })
    if (scenario === 'deferred') w.state.action = output({ outcome: 'deferred', reason: 'Wiki lock is busy' }, 3)
    if (scenario === 'invalid') w.state.action = output({ error: 'Invalid branch' }, 2)
    if (scenario === 'bad JSON') w.state.action.stdout = 'invalid'
    if (scenario === 'timeout') w.state.fail = 'land'
    await ui.press({ key: 'confirm-land' })
    const message = scenario === 'deferred' ? 'Land: deferred — Wiki lock is busy'
      : scenario === 'invalid' ? 'Land: exit 2 — Invalid branch'
      : scenario === 'bad JSON' ? 'Land: invalid response' : 'Curator action failed or timed out.'
    expect(await ui.find({ type: 'Text', text: message })).toBeDefined()
    expect((await ui.find({ type: 'Text', text: message }))?.props.color).toBe(scenario === 'deferred' ? 'warning' : 'error')
    expect(w.mutations().length).toBe(1)
    expect(w.calls.slice(-2).map(call => call.argv[1])).toEqual(['review', 'injections'])
    expect(await ui.find({ key: 'confirm-land' })).toBeUndefined()
    await ui.unmount()
  })
}

test('pending review count refreshes with main turns and the five-minute clock', async ($, on) => {
  const w = world(on)
  await $.session.start(START)
  w.state.review = output({ items: [DEFERRED] })
  await $.turn.complete(TURN)
  expect(w.statuses.at(-1)).toBe('wiki 0↑ · review 1')
  w.state.review = output({ items: [] })
  await $.turn.complete({ ...TURN, agentId: 'child' })
  expect(w.statuses.at(-1)).toBe('wiki 0↑ · review 1')
  await w.clock.advance(300000)
  expect(w.statuses.at(-1)).toBe('wiki 0↑')
})
