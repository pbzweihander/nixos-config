# Shared wiki mod

A Claude Code 2.1.288 function-hooks mod, named `wiki-mod`. It adds candidate wiki
pages to non-fork subagent prompts and skill prompts, pins session usage and
curator counts under the prompt, and provides `/wiki` and `/wiki-review` on
terminal and desktop.

The `claudeWikiBin` string option defaults to `claude-wiki`; `wikiCuratorBin`
defaults to `wiki-curator`. Each accepts a binary path, including paths with
spaces, as one argv entry. For example, the session's plugin options can contain:

```json
{
  "claudeWikiBin": "/path/to/freshly-built/claude-wiki",
  "wikiCuratorBin": "/path/to/freshly-built/wiki-curator"
}
```

Every CLI call goes through `$.process.run`. Read queries and discard have a
4000 ms timeout; land has 150000 ms to accommodate the curator's roughly
130-second wiki-lock wait. The mod does not access the daemon directly.

Related-page queries send only the first 8000 characters of the subagent task
through stdin; the original task is preserved in full. Failed, malformed,
truncated, and empty responses add nothing. Forks bypass the query. The counter
records subagent prompts augmented this session.

The `skill.prompt` hook calls `claude-wiki skill-pages <skill> --json -n 5 --cwd
<session cwd> --session <id> --source skill-prompt` and appends returned paths and
titles as candidates, with an explicit reminder that they are not instructions.
It passes plugin-qualified names such as `plugin:skill` in full. The `claude-wiki`
skill, including a plugin-qualified `claude-wiki`, is excluded. The CLI owns
session deduplication and seen-page tracking; the mod queries each other skill
invocation. Failures and empty results leave the skill text unchanged.

The status refreshes on startup, main-loop completion, every five minutes, and
when either pane opens or a curator action finishes. `wiki N↑` counts the CLI's session injection records, `sub N`
counts augmented subagent prompts, and `curator MM/DD N✓N✗` shows the timestamp's
month/day and the merged/rejected task counts. These counts come from one
`- merged:` or `- rejected:` line per task in `latest.md`; indented detail lines
do not count. `review N` reports pending items from `wiki-curator review --json`.
Zero subagents, zero pending items, and an absent curator summary are omitted.
Failed refreshes preserve previously fetched data. The summary path is derived
from `$.env.get('HOME')`.

In `/wiki`, select a suggestion or submit whitespace-separated search terms. Page
previews use `$.fs.read`, remove opening YAML frontmatter, and display up to 8000
characters as Markdown. Insert path and Insert link insert at the prompt cursor.
Links use the CLI's canonical relative path (`pages/knowledge/build.md` becomes
`[[knowledge/build]]`). Close dismisses the pane; Escape also closes it. Drawing
values live in declared session state, and late search/preview responses cannot
replace newer selections.

Open `/wiki-review`, or press **Review curator** in `/wiki`, to browse rejected
and deferred curator items. Each entry shows its type, outcome, summary or
reason, and page count. Selecting an entry runs `wiki-curator show <id>` and
shows its reason, Codex problems, and diff. Diff previews are capped at about
60 KB of UTF-8 text. Short unified diffs use `Code` with `format="diff"`; longer,
truncated, or non-hunk output uses `Text` with bounded children to respect this
build's 10000-character element limit.

**Land** is available only for branch items and invokes `wiki-curator land
<branch> --json`. **Discard** invokes `wiki-curator discard <id> --json` for
either item kind. Neither first press executes a command: it reveals a separate
**Confirm Land** or **Confirm Discard** button and **Cancel**. Only that confirm
press starts the action. Confirmation is bound to the item and its metadata,
consumed atomically, and cleared on selection changes, changed/removed items,
close, or hot reload. Repeated presses cannot start the same confirmed action
twice. Mutation controls stay unavailable while an action runs.

The pane displays the CLI outcome and reason, including deferred/busy and
invalid-item responses, then refreshes its list and the status line. It does not
retry mutations automatically. A timeout or reload calls for checking the
refreshed list before attempting another action. **Refresh** retries a failed
list query; selecting an item again retries its diff. All drawing values,
including confirmations and progress, live in the declared session state.

From this directory, the requested checks are:

```sh
claude plugin validate "$PWD"
claude plugin test "$PWD"
nix shell nixpkgs#typescript --command tsc -p "$PWD"
```

Tests mock the CLI, filesystem, environment, and clock using `claude-code/testing`;
they do not access the wiki, land/discard real proposals, or launch subagents.
They cover both panes on terminal and desktop, confirmations and concurrent
presses, delayed/stale responses, CLI failures and deferred actions, status
refreshes, subagent injection, and skill injection. The tsconfig follows this
build's API header and includes the locally generated `.claude-plugin/types`
declarations.

The supplied `plugin-authoring/SKILL.md` was absent. Implementation follows its
available `reference.md`, full declaration file, and pane/band/tool-call examples.
