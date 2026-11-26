# Event reminders

`agent-wiki remind --event` accepts hook JSON on stdin. It uses private `state.sqlite` outside the Markdown repository
and the interval reminder's session ID, prompt count, and transcript cursor.
The ordinary `agent-wiki remind` path and semantic suggestions are unchanged.

The input uses common `session_id`, `transcript_path`, `hook_event_name`,
`tool_name`, `tool_input`, and `tool_response` fields, tolerating unknown fields
such as `cwd`. A provider adapter can normalize its shell tool name to `Bash` and
its success result to a `tool_response` object without changing the detector.
Missing session IDs and unsupported events do not produce reminders.

- A successful `PostToolUse` Bash command can trigger `git commit`, `gh pr create`,
  or `gh pr merge` reminders. PostToolUse is the success signal; explicit nonzero
  exit codes, error flags, interruption, and background execution reject it.
  Real Claude transcripts have `stdout`, `stderr`, and `interrupted` in Bash
  results, often without an exit code. Stderr alone is not a failure signal.
- `PostToolUseFailure` counts non-interrupted tool failures of any tool. After
  `AGENT_WIKI_REMIND_FAILURES` failures (default 3, `--failures` overrides), a
  successful commit or test command triggers a resolved-debugging reminder.
  Other successful tools do not reset this count; a successful commit/test or a
  wiki sync does. This is a heuristic, not proof that the failures were related.
- Recognized tests: `cargo test` (including `+toolchain`), `go test`, `pytest`,
  `python -m pytest` / `unittest`, `ctest`, npm/pnpm/yarn/bun `test` or `run test`
  (including `test:*` scripts), and make/just/deno/dotnet `test`.
- Command parsing accepts quoted arguments, environment assignments, common
  git/gh global options, executable paths, and `&&` chains. It deliberately skips
  shell scripts, pipelines, substitutions, redirections, and other control flow
  whose final success would not prove the relevant command succeeded. Help,
  dry-run, compile-only tests, and `gh pr merge --auto` are skipped.

A session window starts on first use and ends when the next interval reminder
fires (normally after 15 prompts). Each event kind is emitted at most once in
that window. Multiple kinds on one tool call share one message. An event reminder
resets the interval prompt count without clearing the set of seen event kinds.
A `agent-wiki sync` observed since the previous reminder suppresses all event
reminders until the next interval reminder; it also resets the prompt count and
failure streak. The interval path keeps its existing transcript sync detection.
The event path additionally recognizes a successful sync directly from a hook,
so it works before that call has been appended to the transcript.

PostToolUse output is JSON `hookSpecificOutput.additionalContext`. PreCompact
output is plain text: according to the [Claude Code hooks reference](https://code.claude.com/docs/en/hooks#precompact-output),
stdout becomes instructions for the compaction summary. It asks to preserve
unwritten findings so the resumed agent can record them; it cannot force a wiki
write before compaction. Both manual and automatic compaction use this path.

Event handling skips wiki layout/index refresh and semantic queries, takes the
existing wiki lock without waiting, and gives SQLite a 50 ms busy timeout. Stdin
is capped at 256 KiB; transcript scans are capped at 4 MiB and checked against a
100 ms budget between lines. Incomplete scans save their cursor and suppress the
current nudge rather than guessing whether a sync occurred. Partial JSONL lines
are retried. The hook has a two-second outer timeout. Malformed input, contention,
and other errors retain the CLI's exit-zero hook behavior; nothing blocks a tool.
