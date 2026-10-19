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
do not count. `review N` reports pending branches from `wiki-curator review --json`:
Codex rejections that adjudication did not land (including adjudication errors and
failed revisions), plus deferred commits. Other gate or Claude failures retain no
candidate and do not increase `review N`; their `latest.md` entries name the failing
gate, give its reason, list up to 20 missing preservation tokens as JSON with a count
of the rest, and print every recall problem. Full details remain in the run JSON.
Diff artifacts are no longer written or listed, and old files are ignored. The mod
accepts only `kind: "branch"`; legacy `kind: "diff"` items are invalid.
Zero subagents, zero pending items, and an absent curator summary are omitted.
Failed refreshes preserve previously fetched data. The summary path is derived
from `$.env.get('HOME')`.

Both panes use colored header badges, numbered selection controls, a bold `▶`
marker for the selected row, rounded detail cards, and a row of actions. The
theme keys used are `claude`, `permission`, `success`, `error`, `warning`, and
`inactive`; the border style is `round`. This build types colors and border
styles as strings rather than enumerating their names; the rendering tests
exercise these concrete values on terminal and desktop.

In `/wiki`, select a suggestion or submit whitespace-separated search terms.
Suggestion sources have distinct badges: remind in warning, recall in permission,
subagent in claude, and skill-prompt in success. Search results separate the title,
colored type, and dim date. The first nine page rows share unique `1`–`9` shortcuts
across suggestions and search results. Page previews have a titled border, use
`$.fs.read`, remove opening YAML frontmatter, and display up to 8000 characters as
Markdown. Insert path (`p`) and Insert link (`i`) insert at the prompt cursor.
Links use the CLI's canonical relative path (`pages/knowledge/build.md` becomes
`[[knowledge/build]]`). Close dismisses the pane; Escape also closes it. Drawing
values live in declared session state, and late search/preview responses cannot
replace newer selections.

Open `/wiki-review`, or press **Review curator** (`r`) in `/wiki`, to browse
retained Codex rejections and deferred curator branches. Its header shows
rejected/error and deferred/warning badges and the last curator summary. Each row has a plain
numbered button (`1`–`9` for the first nine), an outcome badge, an accent-colored
type, a summary fitted to the available width, and a dim page count. Selecting
an entry runs `wiki-curator show <id>`. Its outcome-colored detail card groups
the summary, type/run date/page count, red reason, warning-colored Codex bullets,
and shortened page names.

Diff rendering splits the complete response at `diff --git` boundaries. It shows
only the commit subject above per-file headers: A in success, M in warning, D in
error, and R in permission. Each parseable file gets its own `Code` with
`format="diff"`, the file path, and complete hunks. Each Code is capped at 10000
characters by omitting whole trailing hunks and showing their count. A single
hunk larger than that limit is counted as omitted. Files with no parseable hunks
(including binary and rename-only metadata) use bounded `Text` children, without
affecting colored hunks in other files. The total preview budget remains about
60 KB of UTF-8 text; omitted files/hunks are reported. Large commit messages and
stat tables do not consume the hunk budget.

**Land** (`l`, primary) invokes `wiki-curator land <branch> --json`.
**Discard** (`d`) invokes `wiki-curator discard <branch> --json`.
Neither first press executes a command: it reveals a separate
warning-colored confirmation box naming the item, a primary **Confirm** (`y`)
button, and **Cancel** (`n`). Land also explains that it cherry-picks onto main.
Only the confirm press starts the action. Confirmation is bound to the item and its metadata,
consumed atomically, and cleared on selection changes, changed/removed items,
close, or hot reload. Repeated presses cannot start the same confirmed action
twice. Mutation controls stay unavailable while an action runs.

The pane displays the CLI outcome and reason in success (merged/discarded),
warning (deferred), or error colors, then refreshes its list and the status line. It does not
retry mutations automatically. A timeout or reload calls for checking the
refreshed list before attempting another action. **Refresh** (`r`) retries a failed
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
They cover both panes on terminal and desktop, style props and hotkeys,
per-file diff splitting and hunk-boundary limits, confirmations and concurrent
presses, delayed/stale responses, CLI failures and deferred actions, status
refreshes, subagent injection, and skill injection. The tsconfig follows this
build's API header and includes the locally generated `.claude-plugin/types`
declarations.

The supplied `plugin-authoring/SKILL.md` was absent. Implementation follows its
available `reference.md`, full declaration file, and pane/band/tool-call examples.
