# Codex wiki hooks

Home-manager merges wiki hooks into existing `~/.codex` and `~/.pcodex` homes.
It does not create an unused account directory. `pcodex` selects the latter with
`CODEX_HOME`; Orca's Codex workers inherit its hook configuration as well.

| Event | Command | Output |
| --- | --- | --- |
| SessionStart | `claude-wiki context --harness codex` | Wiki context |
| UserPromptSubmit | `claude-wiki remind --harness codex` | Related pages and write reminder |
| PostToolUse, matcher Bash | `claude-wiki recall --harness codex` | Pages matching a shell failure or diagnostic |

Stop runs `claude-wiki record` detached using the common `session_id` and
`transcript_path` payload fields; a null transcript fails open. PostToolUse/Bash
also runs `claude-wiki remind --event --harness codex`. Event reminders require
explicit status zero (structured status or a status header); raw output without
status never triggers a commit/PR reminder.

Codex 0.155.1 does have PreCompact (`core/src/compact.rs` and
`hooks/src/schema.rs` in the packaged source). Its output supports UI messages
and stop control only: plain stdout is ignored and additionalContext is not
consumed. No PreCompact hook is installed because it cannot carry the Claude
compaction-summary instruction; stopping compaction is not appropriate here.

## Verified contract

Verified against installed **Codex CLI 0.155.1**, its generated app-server schema,
and the exact Nix package source (`codex-rs/hooks/src/schema.rs`,
`core/src/tools/context.rs`, `core/tests/suite/hooks.rs`). The flake's
`multiverse.instance.tip.codex` is also 0.155.1; activation uses that package, not
the older stable `pkgs.codex`.

- Codex reads `$CODEX_HOME/hooks.json` (default `~/.codex/hooks.json`), system
  `/etc/codex/hooks.json`, trusted project `.codex/hooks.json` layers, and enabled
  plugin hooks. It can also load hook definitions from `config.toml`.
- Payload fields overlap Claude Code: `session_id`, nullable `transcript_path`,
  `cwd`, `hook_event_name`, `prompt`, `tool_name`, `tool_input`, `tool_use_id`.
  Codex adds fields including `model` and `turn_id`. Both use PascalCase event
  names, so the command explicitly selects the harness instead of guessing.
- SessionStart and UserPromptSubmit accept plain stdout as developer context.
  PostToolUse ignores plain stdout. We emit a single JSON object for all Codex
  context, with `hookSpecificOutput.hookEventName` and `additionalContext`.
  Claude's existing plain context/reminder output and recall JSON are preserved.
- Exit 0 with empty output succeeds. Exit 2 is blocking feedback on supported
  events; other nonzero hook exits are hook failures. Wiki hooks keep their
  existing fail-open behavior and never intentionally block a session.
- Reminder sync detection accepts rollout JSONL `response_item` / `function_call`
  entries for `exec_command`, `shell`, and `shell_command`, including namespaced
  tool names and stringified JSON arguments containing `claude-wiki sync`.

See the official [hook reference](https://learn.chatgpt.com/docs/hooks) and
[global instruction discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md).

## Shell status limitation

In 0.155.1, `ExecCommandToolOutput::post_tool_use_response` emits only the raw
truncated stdout/stderr string. Its model-facing output has a status header,
but that header is absent from the hook payload. There is no separate
PostToolUseFailure event. The current function-call output is not yet persisted
when PostToolUse runs, and legacy rollouts do not persist ExecCommandEnd, so
looking at the transcript cannot reliably recover this call's status.

Recall first honors structured `exit_code` (including `metadata.exit_code`) or
the exact header lines `Exit code: N` / `Process exited with code N` before
`Output:`. Zero is always silent. Without status, it considers only anchored
diagnostic lines such as `error: ...`, `fatal: ...`, `RuntimeError: ...`, or
`bash: cannot ...`, then applies the existing rare-error-phrase matching.
Code snippets, quoted strings, prose mentioning errors, and filename/line-number
grep matches are excluded. ANSI colors are stripped.

This fallback can still misidentify successful output that reproduces an actual
diagnostic. Its message explicitly says **exit status unavailable**. A normalized
line can cause at most one emitted fallback recall per session, and a session
can receive at most **three** fallback recalls total. Missing session IDs disable
the fallback. Normalization folds case, whitespace, and numeric IDs; the database
stores only deduplication hashes, not diagnostic text. Known-status failures retain
the existing page deduplication behavior and do not consume the fallback budget.

## Mutable files and trust

`merge.py` owns commands starting with `: claude-wiki-hook;`. It replaces those
definitions in place and appends new ones, preserving every foreign group and
handler index. Obsolete owned groups become empty groups; obsolete owned handlers
in mixed groups become disabled no-ops so later foreign trust keys do not move.

Codex skips untrusted hooks. Trust keys are
`<hooks.json path>:<snake_case event>:<group index>:<handler index>`, and
`trusted_hash` approves the exact normalized definition. The installer asks the
installed Codex's `hooks/list` API for hashes in a fresh temporary home containing
only generated wiki hooks and a file-only auth setting. It copies no credentials,
executes no hooks, and grants trust only to those wiki definitions. It never uses
`--dangerously-bypass-hook-trust` or approves foreign hooks. This avoids a manual
prompt on 0.155.1; a manually edited definition can be reviewed using `/hooks` or
restored by another activation.

The existing JSON/TOML is validated before any destination is written. TOML comments
and foreign settings survive. Each changed file gets a `.claude-wiki.bak` backup
and an atomic replacement retaining its mode; unchanged files are not written.
Symlinked destinations and detected concurrent edits are refused.

Orca 1.4.269's installed `managed-agent-hook-controls-3YAGY7kh.js` (`Je`, `Qe`)
and `managed-hook-script-write-BH8euvkU.js` filter only Orca commands, preserve
unknown hooks, and retain its existing slot when replacing the current hook.
Its trust-promotion code remaps foreign trust when its own pruning moves slots.
Thus the wiki entries survive subsequent normal Orca installation/mirroring;
no watcher or rewrite loop is needed.

The same wiki rules are appended to `$CODEX_HOME/AGENTS.md` inside an owned HTML
comment block. Other text remains intact. A nonempty `AGENTS.override.md` takes
precedence in Codex; users opting into that override must include their desired
wiki rules there. The skill is installed at `~/.agents/skills/claude-wiki` for both
accounts. Read-only inspection on 2026-11-23 found mutable `.codex/hooks.json` and
`.codex/config.toml`, no global `AGENTS.md`/`AGENTS.override.md`, no existing
`.agents/skills/claude-wiki`, and no `.pcodex` directory.

## Verification, 2026-11-23

Rust tests include synthetic Codex payloads for all three commands, exact JSON
output, explicit success/failure statuses, raw diagnostics, normalization/caps,
and sync detection resetting the reminder counter. `test_merge.py` checks foreign
positions, backups, idempotence including mtimes, malformed JSON/TOML, symlinks,
and preservation of user instructions and trust entries.

A real `codex exec --skip-git-repo-check --json 'Reply only with OK.'` ran with
fresh temporary HOME and CODEX_HOME, generated hooks/trust config, an empty test
wiki, `CLAUDE_WIKI_REMIND_INTERVAL=1`, and no copied credentials or auth environment.
Codex `hooks/list` reported all three hooks `trusted`, without a trust bypass.
The rollout recorded these exact texts in separate **developer** messages tagged
`hooks.additional_context`:

```text
Shared wiki: 0 pages under /tmp/wiki-codex-e2e-_jtfk61n/wiki/. Search it with `claude-wiki search <terms>` before non-trivial work; load the claude-wiki skill before writing to it.
[context 207/6,000 chars]
```

```text
claude-wiki: 1 prompts since this session last ran `claude-wiki sync`. If this session has produced something another session would need (a root cause and its fix, a tool or environment quirk, a decision and its reason, out-of-scope work for later), record it in the wiki now (load the claude-wiki skill); otherwise continue and do not reply to this note.
```

The model request then failed with `401 Unauthorized: Missing bearer or basic
authentication in header` and `turn.failed` (exit 1). Therefore no model-driven
shell execution was claimed: recall was verified by invoking the real binary
with synthetic Codex payloads. All test wiki writes were confined to temporary
directories; real account configuration and wiki pages were not modified.
