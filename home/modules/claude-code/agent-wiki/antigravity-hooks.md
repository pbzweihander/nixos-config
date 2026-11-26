# Antigravity lifecycle hooks

`--harness antigravity` accepts the camelCase protojson contract in the installed
`agy-customizations/docs/hooks.md`. Home Manager merges the named `agent-wiki`
entry into `~/.gemini/config/hooks.json`; every other top-level key is retained.
The merger validates before writing, backs up replaced bytes, refuses symlinks,
checks for concurrent edits and atomically replaces the mutable file. Backups
(`hooks.json.agent-wiki-backup-<digest>`, mode 0600) are named by a digest of the
keys the merger does not own, so a rebuild that only changes the agent-wiki store
path reuses the existing backup instead of adding one per build. Hook commands
use the built agent-wiki Nix store path. The shared skill is also installed at
`~/.gemini/config/skills/agent-wiki`, Antigravity's documented global skill root.
Building does not activate the hooks.

| Event | Command | Behavior |
| --- | --- | --- |
| SessionStart | context | Project from cwd or first workspace, usage under conversationId |
| PreInvocation | remind | Drain pending notes; prompt processing only on invocationNum 1 |
| PostInvocation | remind | Drain pending notes only |
| PostToolUse (read_file or run_command) | recall; remind --event | Stage notes; stdout is always `{}` |
| Stop | record | Flush `{}` then detach with setsid; never request continuation |

Context replies use `injectSteps[].systemMessage.systemMessage`. Empty replies,
malformed stdin, busy wiki locks and other hook errors produce `{}` and exit zero.
No PreToolUse handler is installed. A direct call with that event returns
`{"decision":"allow"}`, also on error paths (bad flags, fields of the wrong type,
panics) whenever stdin is a JSON object with `hookEventName`. Stdin that is not
JSON, or larger than 1 MiB, has no readable event and produces `{}`.

Every hook, including `remind --event`, waits for the wiki lock up to
`AGENT_WIKI_HOOK_LOCK_MS` (default 2000) like the other harnesses' hooks, capped
at 3000 ms to stay below Antigravity's 5 s hook timeout; it then gives up with
`{}`. Unlike Claude/Codex, `remind --event` waits too, because Antigravity runs it
beside `recall` for the same tool and they would otherwise contend.
`record --wait` is the synchronous diagnostic variant and still reports errors
through its exit status and stderr, with JSON on stdout.

The queue and prompt/tool deduplication tables live in the existing `index.sqlite`,
under the wiki lock. Pending notes expire after a day and tool replay keys after
seven days. Recall and remind share seen-page state; recall injection usage is
logged when drained. A drain selects the queued rows, builds the whole reply
(a failing prompt reminder is logged to stderr and does not block the notes), and
only then deletes exactly the selected rows and logs usage. A failure before that
keeps the notes queued, and a note staged meanwhile is not removed. Delivery is
still best effort, as with the existing hooks: a timeout or a stdout failure after
the commit can lose a note. No hook forces a model
continuation. A final PostInvocation can therefore save a note for the next turn.

Prompt keys hash `initialNumSteps` plus `lastUserInput`, scoped by `conversationId`.
Prompt and tool replay keys are FNV-1a 64 over JSON, stable across Rust releases.
Only `invocationNum == 1` can advance the 15-prompt counter or call wiki-embed.
This deduplicates repeated hook delivery and tool continuations while allowing
identical text on later turns with a different initial step count. If initial step
counts are missing, identical consecutive text is conservatively deduplicated.
Raw prompts/tool inputs are not persisted by the adapter.

Successful `read_file` of `*/skills/<name>/SKILL.md` uses the existing skill-tag
recall. Tool errors use the existing exact error-line matching. The observed
transcript format differs from hooks: rows have `step_index`, `created_at`, `type`
(`USER_INPUT`, `PLANNER_RESPONSE`, `GENERIC`) and `status` (`DONE`, `ERROR`).
`run_command` results put `Command exited with code N.` in a header after the
`Created At`/`Completed At` lines (repeated inside the preview of truncated
results), followed by `Command output:` and the raw output. Only the header lines
before the first `Command output:` line are searched for that anchored line, so
long output never hides it and output text cannot fake it; content without a
`Command output:` line is all header. Only the
completed GENERIC row matching the hook's `stepIdx` is read, within the last 4 MiB
and a 100 ms scan budget. Hook `error` takes precedence. Missing status uses the
existing anchored diagnostic fallback with deduplication and a three-note budget;
it cannot establish a successful commit/test event. Background completions or a
transcript not yet flushed at hook time may therefore be missed.

Stop records keep prompts, observed tool counts, timestamps and explicit failures
in private `records.sqlite`. Planner `tool_calls` have no IDs in the inspected
transcript, so generic results are not guessed to belong to particular commands.
Successful file writes/commit/PR associations are deliberately absent in these
records. Unknown rows and incomplete/malformed JSONL lines are ignored. Curator
marker/cwd exclusions and credential redaction still apply. Search with
`agent-wiki sessions --records --harness antigravity`; raw transcript indexing and
Antigravity backfill are not implemented.

Verification: `cargo test`, plus
`python3 -m unittest discover -s ../antigravity-hooks -v` from this directory.
Integration tests use temporary wiki/state/config directories and synthetic rows
matching the inspected schema; they do not launch a model or edit live hooks.
