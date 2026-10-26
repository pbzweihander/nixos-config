# wiki-curator

Daily, conservative maintenance for the shared `claude-wiki` git repository. On
rossmann the user timer starts at 05:00 local time, with up to 15 minutes of random
delay and catch-up after downtime. It attempts at most five tasks per run.

The job reads `claude-wiki lint --json` and considers, in order:

1. Broken links, one page per task.
2. Candidate duplicate knowledge pages or two open follow-ups with similarity at
   least 0.90. Claude first checks that they describe the same topic, system and
   facts, rather than different versions, dates, rounds or machines. Related pages
   need not be duplicates: abstain by changing no page and writing a justification
   whose summary starts `not duplicates: <reason>`, with empty `dropped` and
   `redirects`. For true duplicates, keep the earlier `created` date; break ties by
   backlink count, then path. Unknown creation dates rank last. Merge facts, remove
   the other page, update its links. Pairs containing a dated knowledge slug or any
   page in lint's `snapshots` category are excluded, even with `--only duplicates`;
   snapshot tasks handle those pages instead.
3. Dated knowledge snapshots, moved with `git mv` to dated history paths. Dates
   anywhere in the slug are moved to the front; existing destinations are skipped.
   A page tagged `decision` is renamed within knowledge without the date instead,
   because history is excluded from recall and decision records must stay findable.
4. Log-like knowledge or follow-up pages: present state in the page, timeline in a
   new dated history entry, with links both ways.
5. Missing queries, one batch of up to five knowledge pages.
6. Singleton tags, one batch of up to twenty tags: replace one-for-one with suitable
   existing tags, or leave them.

Within each type, candidates with higher lint usage importance come first. Duplicate
pairs use the greater importance of their two pages. Ties keep the existing path
order (or duplicate similarity, then paths); missing usage counts as zero. Usage
also determines which pages/tags fit into batches. Type priority and task/batch
limits are unchanged. Importance only orders work; it is never a deletion criterion.

Projects, dirty pages, pages modified within 24 hours, and pages on cooldown are
skipped. Necessary backlink updates and new destinations are part of a task's page
set, so a protected backlink skips the whole merge or move. A page participates in
at most one task per run. There are no stale-follow-up, orphan, or unread tasks.

Every duplicate task that finishes as a no-op suppresses that pair for 90 days,
including no-ops without a valid justification. The justification's summary, if
present and readable, is recorded as the task's reason. Suppression applies only to
that pair, independent of its order in lint; both pages remain eligible for other
tasks and other pairs. Once the deadline expires, the pair is eligible again.

## Isolation and gates

Each run owns a detached worktree in the cache. Before each task it checks out the
latest `main` and cleans untracked files. Claude runs there with wiki hooks disabled,
no session persistence, an explicit editing-tool allowlist, a 40-turn limit and a
20-minute timeout. It receives a short copy of the wiki's writing rules and must
write `.curator/justification.json` with a summary, explained dropped tokens, and
redirects. The semantic daemon socket is disabled for worktree CLI commands;
the recall gate separately requests dense ranks with page-text overrides.

Before committing, the curator checks scope, redirects and required link updates,
rejects new broken wiki links, and runs `claude-wiki check`. For each added or
modified page, missing `[[type/slug]]` targets are compared with those already
missing in that same page at the parent. Existing broken links are tolerated;
new ones are recorded in the `broken-links` gate and reject the attempt.
Blocking validation errors, unexpected exit codes,
non-regular files, protected pages, unplanned edits, missing/invalid justification,
and failed Claude invocations reject the attempt. Ordinary validation warnings are
allowed; query tasks require exit **0** from `check --probe`. Empty attempts are
no-ops. `.curator/` is never committed.

For singleton-tags tasks, a deterministic `singleton-tags` gate also requires every
changed page's body to remain byte-identical and its tag count not to decrease
(duplicates cannot make up for lost tags). Each disappeared tag needs a distinct replacement that existed on another
wiki page at the original parent; new tags are forbidden. Failures list the dropped
tags per page. The parser accepts ordinary flow/block lists and fails closed on
unsupported metadata syntax.

Four more gates apply to the resulting candidate commit, in this order:

- **Preservation:** compare the parent bodies of modified/deleted/renamed pages to
  the union of new bodies of changed pages. Inline code, numbers/versions/units,
  URLs and wiki-link targets must survive. The gate also preserves absolute and `~/`
  paths with at least two segments, filenames ending in `rs`, `py`, `nix`, `toml`,
  `json`, `yaml`, `yml`, `md`, `ts`, `tsx`, `js`, `go`, `sh`, `fish`, `sql`, `conf`,
  `service`, or `timer`, UPPER_SNAKE identifiers, snake_case identifiers with letters
  in every component and at least one component containing two letters (excluding
  prose notation such as `a_b`), `E` plus 2–4 digits, and `HTTP` plus 3 digits.
  These patterns apply in prose, inline code and fenced code; trailing punctuation
  is stripped. Link targets remain subject to redirect normalization rather than
  identifier matching. Fenced code contents are included;
  frontmatter is excluded. Ordered-list marker numbers (`1. ` or `2) ` at line
  start, with optional indentation or blockquote `>`) are ignored outside code
  fences; other numbers on the line still count. Links are normalized using Git's `-M` rename detection
  and the planned redirects. Only exact tokens explicitly listed in `dropped` are
  exempted. This checks token presence, not meaning or how often a fact occurs.
  Error lines are also preservation tokens: fenced lines containing `error[E123]:`
  (any digit count), a capitalized name ending in `Error:` or `Exception:`,
  `panicked at`, leading `fatal:`, or `Traceback (most recent call last)`, and
  unfenced lines that start with one of them (optionally after `thread 'name' `).
  Prose that only mentions an error is not a token, so rewording it is not a loss;
  its inline code spans still are. Matching is case-sensitive. Leading/trailing
  whitespace, leading blockquote/list markers and surrounding backticks are
  stripped, and the first 128 characters form the token.
- **Invented tokens:** after preservation passes, compare new body tokens against
  the union of old tokens using the same link normalization. A token is also
  supported if its text, with surrounding backticks removed, occurs anywhere in
  a changed page's old full text, including frontmatter. Wrapping an existing
  identifier in backticks therefore does not invent it. Wiki-link tokens are
  excluded because the broken-link gate covers them. `dropped` exceptions do not
  excuse inventions. `WIKI_CURATOR_INVENTED_GATE=report` (the default) records
  `invented_tokens` without rejecting; `reject` rejects nonempty lists with
  `invented-token check failed`. Any other value makes the task an error.
- **Recall:** probe every parent-frontmatter query of modified, deleted or renamed
  pages, plus queries of added pages. Expected pages follow Git's `-M` renames and
  justification redirects. Deleted pages with queries need a surviving target;
  moved or merged queries must appear there as exactly the same strings.
  Temporarily check out the parent, then restore the candidate in a `finally`
  block and rank both versions with `search --json --no-semantic -n 10`, splitting
  queries into whitespace-separated arguments. Send one `wiki-embed` `probe`
  request with distinct queries, `n=10`, and a 120-second timeout. Its `parent`
  and `commit` states override changed pages with their git-object texts and
  remove paths absent in each state, including both ends of renames. Dense
  ranking does not write to the worktree or live wiki. A probe in either method's
  parent top 3 must remain in either method's commit top 3. Existing failures
  do not reject; added pages have no parent rank. The gate records each probe's
  page, expected page, query, BM25 `parent_rank`/`commit_rank` and dense
  `parent_dense`/`commit_dense` (null means absent from the top 10).
  Successful dense requests record `dense: "ok"`. With no configured socket,
  connection errors, timeouts, daemon errors or malformed responses, the gate
  remains BM25-only and records `dense: "unavailable: <reason>"` plus `ungated`,
  the probes without a BM25 parent rank. Dense unavailability does not reject.
  The stdlib parser supports ordinary inline and block lists of plain,
  single-quoted or JSON-quoted strings; unsupported query syntax and search errors
  reject the attempt. CLI searches use the isolated worktree and disabled daemon.
- **Codex:** a read-only review receives the task, justification, writing rules and
  full old/new page texts, together with all gate results including recall ranks.
  It checks every fact, distinct versions/dates/machines,
  provenance, inventions, scope and page types. Dropped-token exceptions still need
  semantic approval. Query wording/form alone cannot reject a change unless the
  query is a bare keyword list, unrelated to its page, or false. Tag preference
  alone cannot reject when the tag gate passed. Such style notes may appear in
  `problems` with `accept: true`. A strictly valid final-line JSON acceptance passes;
  rejection, nonzero exits, malformed/missing output or a 15-minute timeout go to
  adjudication when every deterministic gate passed and the worktree is intact.

By default, a single Claude adjudicator receives the task, justification, all gate
results, Codex problems, and full before/after texts. It uses the same settings
isolation, model and timeout as the editing call, with no tools, one turn and no
session persistence. Its final-line JSON contains `verdict`, a Korean `reason`, and
`edits` (objects with `path`, `old`, `new`):

- **override:** land through the normal guarded landing function only when every
  page body is byte-identical at the same path. A body change, addition, deletion or
  move converts the verdict to uphold, retaining the requested verdict in the report.
- **revise:** validate at most ten nonoverlapping exact edits against the reviewed
  commit. Each nonempty `old` must occur exactly once, and each path must already
  be changed by that commit. Apply the edits in the worktree and rerun scope, broken
  links, check, the tag gate when applicable, preservation, invented tokens, recall and Codex. The
  replacement candidate has the original parent, so landing includes the complete
  change and retained branches remain compatible with manual `land`. Only a passing
  second review can land; there is never a second adjudication.
- **uphold**, unusable output, invalid edits, or any failed revision: retain the
  rejection and normal cooldown. The branch keeps the latest candidate commit;
  failures before a replacement commit keep the original candidate.

`gates.adjudicate` records the verdict, Korean reason, edits, errors, original commit,
revision gates and second review when present. `latest.md` and `wiki-curator review`
show the reason below the original Codex problems, followed by edits and the second
review. Successful summaries say `merged (adjudicated: override|revise)`.
Set `WIKI_CURATOR_ADJUDICATE=0` to disable this stage.

To land, the job takes the wiki's existing `flock(2)` lock for at most 120 seconds,
verifies that its checkout is on `main` with no Git operation in progress, rechecks
the touched paths, and runs `git cherry-pick -x`. Unrelated **unstaged** edits remain
untouched. Any staged changes defer landing, because cherry-pick requires a clean
index. New backlinks that appeared during review also defer a move/merge. Conflicts
are aborted and deferred. The curator never resets, stashes, checks out, cleans,
or stages files in the main checkout. Git hooks and commit signing are disabled
for curator Git commands.

Rejected candidates are retained as `curator/rejected/<UTC timestamp>-<task number>`
only when they have a commit and a recorded Codex rejection that adjudication did
not land, including adjudication errors and failed revisions. Claude failures,
deterministic gate failures (scope, justification, check, broken links, tags,
preservation, invented tokens and recall), and interruptions or unexpected errors before Codex
create no branch or diff artifact. No `runs/*.diff` files are written; existing
files are ignored. Inspect the run report for these failures before the pages
become eligible again after their cooldown.

Rejected/error tasks cool down all their target pages for fourteen days; deferred
tasks for one day. Deferred commits are
also retained, as `curator/deferred/<UTC timestamp>-<task number>`. Accepted commits
have a `curator: <type>: <summary>` subject and a `Curator-Run` trailer.

## Run and inspect

```sh
wiki-curator run
wiki-curator run --max-tasks 2 --only broken-links missing-queries
wiki-curator run --wiki /path/to/test-wiki
wiki-curator status
systemctl --user status wiki-curator.timer wiki-curator.service
journalctl --user -u wiki-curator.service
```

The wiki defaults to `~/.local/share/claude-wiki`. Environment settings:

| Variable | Meaning |
| --- | --- |
| `CLAUDE_WIKI_DIR` | Wiki root; `--wiki` takes precedence. |
| `WIKI_CURATOR_MAX_TASKS` | Default task limit (five); `--max-tasks` takes precedence. Zero selects no tasks. |
| `WIKI_CURATOR_MODEL` | Optional Claude model, passed as `--model`. |
| `WIKI_CURATOR_ADJUDICATE` | Set to `0` to disable adjudication of Codex rejections; enabled otherwise. |
| `WIKI_CURATOR_INVENTED_GATE` | `report` (default) records invented tokens; `reject` rejects them. Other values are task errors. |
| `CLAUDE_WIKI_SOCKET` | Dense recall socket; an explicitly empty value disables dense recall. CLI commands in the worktree always receive an empty value. |
| `XDG_RUNTIME_DIR` | When `CLAUDE_WIKI_SOCKET` is unset, dense recall uses `$XDG_RUNTIME_DIR/claude-wiki/embed.sock`. Without either variable, dense recall is unavailable. |
| `XDG_CACHE_HOME` | Cache root, default `~/.cache`. State is in its `wiki-curator/` directory. |

The service PATH includes `~/.local/bin`, the home profile's `bin`, and
`/run/current-system/sw/bin`. Claude and Codex must already be authenticated for
unattended use. Set service environment overrides with `systemctl --user edit
wiki-curator.service`, under `[Service]` using `Environment=NAME=value`.

`run.lock` excludes concurrent runs; another invocation exits successfully right
away. `runs/<UTC timestamp>.json` records tasks, outcomes, gate results, Codex
problems, candidate/landed SHAs and retained branches. `latest.md` names each
failing gate (`gate: preservation`, for example), gives its reason, lists missing
preservation tokens as a JSON list (`missing: [...]`, first 20 plus a count of the
rest), and prints every recall problem, including both methods' before/after ranks.
For every task, including merged ones, it lists nonempty invented tokens as
`invented: [...]` (first 20 plus a count of the rest). Dense fallback appears as
`dense: unavailable (<reason>); N probes ungated`. The JSON keeps full gate details. Claude
editor and adjudicator failures use the last 500 stdout characters, stripped and
with newlines collapsed, when stderr is empty.

`state.json` stores Unix-time deadlines in `cooldowns` (per page) and
`not_duplicates` (90-day suppression, keyed by two sorted page paths joined with
`|`, for example `pages/knowledge/a.md|pages/knowledge/b.md`). Old state files without
`not_duplicates` load with an empty map. A legacy `dismissed_diffs` key is ignored
and dropped on the next state write. Both maps are validated strictly; invalid
paths, unsorted/repeated pairs, nonnumeric or nonfinite deadlines fail the run.
Reports and state are checkpointed between tasks. A corrupt state file fails the
run rather than bypassing suppression. Normal completion, exceptions and handled
service termination remove the worktree; a later run removes a registered leftover after an abrupt
kill. Task rejection/failure exits zero; run setup or cleanup failure exits nonzero.

## Review retained work

List and inspect candidates without taking `run.lock` or changing the checkout:

```sh
wiki-curator review
wiki-curator review --json
wiki-curator show 'curator/rejected/<timestamp>-<n>'
wiki-curator land 'curator/rejected/<timestamp>-<n>' --json
wiki-curator discard 'curator/deferred/<timestamp>-<n>' --json
```

All four commands accept `--wiki` and use the same wiki/cache environment settings
as `run`. `review` lists every retained rejected and deferred branch, newest first.
Diff artifacts and their former `--days` filter are no longer supported.
Text output gives one line per item,
with Codex problems and any adjudication details indented below it. Reports are joined by timestamp and task
number; missing, unreadable or corrupt report fields become nulls or empty lists.
Branches with nonstandard names within the curator prefixes remain visible, with
null run/task fields and their commit date used for ordering.

For a mod pane, `review --json` returns `{"items": [...]}`. Each item has `id`,
`kind` (`branch`), `branch`, `run` (UTC timestamp),
`task` (one-based number), `type`, `pages`, `outcome` (`rejected` or `deferred`),
`reason`, `codex_problems`, `adjudicate` (the adjudication gate or null), `summary`
(from the justification), and `commit`. Fields
that do not apply are null; page/problem lists default to empty. IDs are exact branch
names, not arbitrary paths or Git revision expressions. There is no `diff` field
or `diff:<timestamp>-<n>` item; `show`, `land` and `discard` refuse those legacy IDs.
`show` prints `git show -M --stat -p` for a branch's current commit. Output is capped
at 1 MiB including `[wiki-curator: diff truncated at 1 MiB]` when truncated.

`land` accepts only `curator/rejected/*` and `curator/deferred/*` branches with
exactly one unmerged commit whose sole parent is an ancestor of `main`. Human
review explicitly overrides the task's rejection/deferral cooldown; it does not
rerun Claude, Codex or the earlier validation gates. Landing shares the service's
wiki lock, checkout/operation verification, protected/recent/dirty page checks,
staged-index check, backlink checks, and `cherry-pick -x` with verified abort on
failure. Removed pages are derived from the commit, so missing reports cannot
bypass backlink checks. Unrelated unstaged work remains intact.

On success, `land` deletes the candidate branch and clears cooldowns for the task's
pages (also using the changed paths when a report is missing). JSON output is
`{"outcome":"merged","landed_commit":"<sha>"}`, exit **0**. A safe deferral keeps
the branch and state, prints `{"outcome":"deferred","reason":"..."}`, and exits
**3**. Invalid IDs and refused branches print `{"outcome":"refused","reason":"..."}`
and exit **2**. Without `--json`, results are plain text.

`discard` deletes a curator branch with `git branch -D`. Reports and cooldowns
remain intact. Successful discard returns `{"outcome":"discarded"}` with `--json`,
exit **0**; invalid input exits **2**.

Both `land` and `discard` take `run.lock` without waiting. While a run or another
mutation holds it, they exit **3** with reason `curator run in progress`. The
service's concurrent-run behavior remains a successful skip. Inspection works
while a run holds the lock.

For debugging, run inside a wiki repo:

```sh
wiki-curator check-preservation <parent> <commit> --justification /path/to/justification.json
```

It prints `{"missing_tokens": [...]}` and exits one on missing tokens or invalid
input. Without `--justification`, no dropped-token exemptions or explicit redirects
apply. The justification also lives in the candidate's commit body and run JSON.

## Build and tests

To replay preservation and invented-token checks against historical `curator:` commits,
run from this configuration repository with an explicit wiki repository path:

```sh
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=home/modules/claude-code/wiki-curator \
  nix shell nixpkgs#python3 nixpkgs#git -c python3 -m wiki_curator.replay /path/to/wiki
```

The helper reads history reachable from HEAD; add `--all` to include all refs,
including retained rejected/deferred candidates. Each commit is compared with its
first parent using the `Justification: {...}` line from its message body. It prints
failing commit IDs, subjects and missing tokens (or malformed-justification errors),
and `invented: [...]` for every commit with new tokens, then counts of failures and
commits with invented tokens. Exit status still depends only on missing tokens and
errors: 1 when any fail, otherwise 0. It never checks out a
revision, runs models, updates usage, or writes the repo/cache. This is an audit of
token presence, not a replay of the semantic reviewer. Run it yourself on the wiki
you want to inspect.

The package uses only Python's standard library and Git. Its Nix `checkPhase` runs
the unittest suite with Git on PATH. All tests use temporary repositories and fake
Claude, Codex and wiki executables; they never access the live wiki or run models.

From the configuration repository:

```sh
nix shell nixpkgs#python3 nixpkgs#git -c python3 -m unittest discover -s home/modules/claude-code/wiki-curator/tests -v
```
