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
6. Singleton tags, one batch of up to twenty tags, reusing existing tags when suitable.

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
redirects. The worktree's semantic daemon socket is disabled.

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

Three more gates apply to the resulting candidate commit, in this order:

- **Preservation:** compare the parent bodies of modified/deleted/renamed pages to
  the union of new bodies of changed pages. Inline code, numbers/versions/units,
  URLs and wiki-link targets must survive. Fenced code contents are included;
  frontmatter is excluded. Links are normalized using Git's `-M` rename detection
  and the planned redirects. Only exact tokens explicitly listed in `dropped` are
  exempted. This checks token presence, not meaning or how often a fact occurs.
- **Recall:** probe every parent-frontmatter query of modified, deleted or renamed
  pages, plus queries of added pages. Expected pages follow Git's `-M` renames and
  justification redirects. Deleted pages with queries need a surviving target;
  moved or merged queries must appear there as exactly the same strings.
  Temporarily check out the parent, then restore the candidate in a `finally`
  block and rank both versions with `search --json --no-semantic -n 10`, splitting
  queries into whitespace-separated arguments. A parent rank of 1–3 must remain
  in the top 3. Existing failures are reported without rejecting the change;
  added pages have no parent rank. The `recall` gate records every probe's page,
  expected page, query and both ranks (null means absent from the top 10).
  The stdlib parser supports ordinary inline and block lists of plain,
  single-quoted or JSON-quoted strings; unsupported query syntax and search errors
  reject the attempt. All searches use the isolated worktree and disabled daemon.
- **Codex:** a read-only review receives the task, justification, writing rules and
  full old/new page texts, together with all gate results including recall ranks.
  It checks every fact, distinct versions/dates/machines,
  provenance, inventions, scope and page types. Dropped-token exceptions still need
  semantic approval. Only a strictly valid final-line JSON acceptance without
  problems passes. Nonzero exits, malformed/missing output or a 15-minute timeout
  reject the attempt.

To land, the job takes the wiki's existing `flock(2)` lock for at most 120 seconds,
verifies that its checkout is on `main` with no Git operation in progress, rechecks
the touched paths, and runs `git cherry-pick -x`. Unrelated **unstaged** edits remain
untouched. Any staged changes defer landing, because cherry-pick requires a clean
index. New backlinks that appeared during review also defer a move/merge. Conflicts
are aborted and deferred. The curator never resets, stashes, checks out, cleans,
or stages files in the main checkout. Git hooks and commit signing are disabled
for curator Git commands.

Rejected candidates with a commit are retained as
`curator/rejected/<UTC timestamp>-<task number>`. When page edits are rejected before
a commit (including scope/validation errors or Claude failure), their patch is saved
as `runs/<UTC timestamp>-<task number>.diff` in the curator cache before worktree
cleanup. The task result's `diff` field and its summary line give the path. The patch
compares the worktree to the task's parent, includes tracked edits/deletions and new
files (including binary files), and includes only `pages/`. Its size is capped at
1 MiB including a truncation marker; a truncated patch is for inspection, not replay.
Empty page diffs do not create an artifact. Capture uses only the disposable
worktree's index; the main checkout and its index remain untouched.

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
| `XDG_CACHE_HOME` | Cache root, default `~/.cache`. State is in its `wiki-curator/` directory. |

The service PATH includes `~/.local/bin`, the home profile's `bin`, and
`/run/current-system/sw/bin`. Claude and Codex must already be authenticated for
unattended use. Set service environment overrides with `systemctl --user edit
wiki-curator.service`, under `[Service]` using `Environment=NAME=value`.

`run.lock` excludes concurrent runs; another invocation exits successfully right
away. `runs/<UTC timestamp>.json` records tasks, outcomes, gate results, Codex
problems, candidate/landed SHAs and retained branches. `latest.md` is the human
summary; `state.json` stores Unix-time deadlines in `cooldowns` (per page) and
`not_duplicates` (90-day suppression, keyed by two sorted page paths joined with
`|`, for example `pages/knowledge/a.md|pages/knowledge/b.md`). Old state files without
`not_duplicates` load with an empty map. Both maps are validated strictly; invalid
paths, unsorted/repeated pairs, nonnumeric or nonfinite deadlines fail the run.
Reports and state are checkpointed between tasks. A corrupt state file fails the
run rather than bypassing suppression. Normal completion, exceptions and handled
service termination remove the worktree; a later run removes a registered leftover after an abrupt
kill. Task rejection/failure exits zero; run setup or cleanup failure exits nonzero.

## Review retained work

List and inspect candidates without taking `run.lock` or changing the checkout:

```sh
wiki-curator review
wiki-curator review --json --days 30
wiki-curator show 'curator/rejected/<timestamp>-<n>'
wiki-curator show 'diff:<timestamp>-<n>'
wiki-curator land 'curator/rejected/<timestamp>-<n>' --json
wiki-curator discard 'curator/deferred/<timestamp>-<n>' --json
wiki-curator discard 'diff:<timestamp>-<n>' --json
```

All four commands accept `--wiki` and use the same wiki/cache environment settings
as `run`. `review` lists every rejected and deferred branch, plus undismissed diff
artifacts whose run timestamp is within the last `--days` days (default 30), newest
first. The age limit applies only to diffs. Text output gives one line per item,
with Codex problems indented below it. Reports are joined by timestamp and task
number; missing, unreadable or corrupt report fields become nulls or empty lists.
Branches with nonstandard names within the curator prefixes remain visible, with
null run/task fields and their commit date used for ordering.

For a mod pane, `review --json` returns `{"items": [...]}`. Each item has `id`,
`kind` (`branch` or `diff`), `branch`, `diff` (an absolute path), `run` (UTC timestamp),
`task` (one-based number), `type`, `pages`, `outcome` (`rejected` or `deferred`),
`reason`, `codex_problems`, `summary` (from the justification), and `commit`. Fields
that do not apply are null; page/problem lists default to empty. IDs are exact branch
names or `diff:<timestamp>-<n>`, not arbitrary paths or Git revision expressions.
`show` prints `git show -M --stat -p` for a branch's current commit, or the diff file's
bytes. Output is capped at 1 MiB including `[wiki-curator: diff truncated at 1 MiB]`
when truncated. Dismissed diffs can still be shown by ID.

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

`discard` deletes a curator branch with `git branch -D`, or adds a diff ID to
`state.json`'s `dismissed_diffs` list so it disappears from `review`. Diff files,
reports and cooldowns remain intact. Successful discard returns
`{"outcome":"discarded"}` with `--json`, exit **0**; invalid input exits **2**.
`dismissed_diffs` defaults to an empty list in older state files. Its entries must
be unique, valid diff IDs with real UTC dates and positive task numbers; malformed
state is refused before mutation.

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

The package uses only Python's standard library and Git. Its Nix `checkPhase` runs
the unittest suite with Git on PATH. All tests use temporary repositories and fake
Claude, Codex and wiki executables; they never access the live wiki or run models.

From the configuration repository:

```sh
nix shell nixpkgs#python3 nixpkgs#git -c python3 -m unittest discover -s home/modules/claude-code/wiki-curator/tests -v
```
