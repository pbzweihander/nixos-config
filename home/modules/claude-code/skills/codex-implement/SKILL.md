---
name: codex-implement
description: Write an implementation spec for a coding task, delegate the implementation to Codex CLI (latest from nixpkgs nixos-unstable via nix, falling back to local codex), then review the result. Use when the user asks to have codex implement, build, or write something, or says "codex한테 시켜", "codex로 구현".
---

> **This skill is managed by Nix.** `~/.claude/skills/codex-implement` is a read-only
> symlink into the Nix store. To change this skill, edit the source at
> `~/nixos-config/home/modules/claude-code/skills/codex-implement/` and run
> `home-manager switch` (or `nixos-rebuild switch`) to apply. Never try to edit the
> files under `~/.claude/skills/` directly.

# Codex implement

You write the spec. Codex writes the code. You review the code.

## 1. Write the spec

Understand the request first: read the relevant parts of the codebase, check existing
conventions, and ask the user only if different readings would lead to materially
different work.

Then write the spec to `<scratchpad>/codex-spec-<slug>.md`. Codex sees only this file
and the repository, so the spec must be self-contained. Include:

- **Goal**: one paragraph, what and why.
- **Context**: repo layout, relevant files with paths, conventions to follow
  (formatter, lint, test command, language/framework versions).
- **Requirements**: numbered, concrete, testable. Describe behavior, not
  implementation, unless the implementation matters.
- **Out of scope**: what codex must not touch or change.
- **Acceptance**: exact commands that must pass (build, test, lint).
- **Rules**: do not commit; do not modify files outside the listed area without
  reason; do not add dependencies unless listed; keep the diff focused.

Keep the spec short enough to read in a few minutes. Show the user the spec path.

## 2. Decide the run flags

Decide these while writing the spec, not after codex fails.

**Model and reasoning effort.** When the user names a model or a thinking/reasoning
effort ("gpt-5.5로", "effort high", "xhigh로 돌려"), always pass it explicitly as a
flag, on the first run and on every resume:

- model: `-m <model>`
- effort: `-c model_reasoning_effort=<effort>`

Pass it even when it matches the current default in `~/.codex/config.toml`. That
default can change, and an explicit flag keeps what the user asked for visible in the
command and in your report. Never pass only one of the two when the user asked for
both.

Supported efforts differ by model. Check them in codex's model cache:

```bash
jq -c '.models[] | {slug, efforts: [.supported_reasoning_levels[]? | .effort // .]}' \
  ~/.codex/models_cache.json
```

If the requested effort is not supported by that model, or the user gave only a vague
phrase ("더 깊게 생각해"), pick the closest supported value and tell the user which
one you used. When the user names neither, omit both flags so the config default
applies.

**Network.** The workspace-write sandbox blocks outbound network by default. Enable
it up front when the task needs it, and say so in the spec:

- Needs network: installing or updating dependencies (`npm install`, `cargo add`,
  `pip install`, `go get`), fetching schemas or fixtures, calling an external API,
  cloning anything.
- Needs network: **any nix command** (`nix develop`, `nix build`, `nix run`,
  `nix shell`, `nix flake check`, `nix eval`, direnv with `use flake`), even when
  everything is already in the store. nix talks to the daemon over a unix socket,
  and with network off the sandbox blocks `connect()` on every socket:
  `cannot connect to socket at '/nix/var/nix/daemon-socket/socket': Operation not permitted`.
  If the project builds or tests through a flake devShell, enable network.
- Also needs network: anything that opens an IP socket, **even on localhost**. With
  network off, the sandbox refuses to create the socket at all
  (`PermissionError: [Errno 1] Operation not permitted`). This covers tests that
  start an HTTP server or router, integration tests against a local database or
  mock server, dev servers, and port checks.
- No network: pure code edits, unit tests that never open a socket, builds whose
  dependencies are already vendored or cached.

If the acceptance commands run tests, check whether they open sockets before
deciding: look for test servers, `listen`/`bind`, `127.0.0.1` or `localhost`, and
test-container or database fixtures.

When needed, add `-c sandbox_workspace_write.network_access=true`. Prefer this
over `--sandbox danger-full-access`, which removes the filesystem sandbox as well.

**Runtime approvals.** `codex exec` is non-interactive: a command that needs to
escalate outside the sandbox normally just fails and the error goes back to codex.
Nobody can answer an approval prompt on codex's behalf, including you. When the
task is likely to need escalation you cannot predict (unusual build tooling, scripts
that write outside the workspace, mixed network needs), add `--approve-for-me`.
A separate Codex reviewer agent then judges each escalation request, allowing
ordinary network and out-of-sandbox commands and blocking credential exfiltration
and destructive actions. `--approve-for-me` already implies the workspace-write
sandbox and **cannot be combined with `--sandbox`**: drop `--sandbox workspace-write`
when you use it.

**Trust.** Directory trust does not gate `codex exec` itself. It only decides whether
codex loads the project's `.codex/` layer: project config, hooks, and
`.codex/rules/*.rules`. Trust is a plain config entry, so no human step is needed.
If you write project rules (see below), pass
`-c 'projects."<absolute project root>".trust_level="trusted"'` for that run, or add
the same entry under `[projects."<absolute project root>"]` in
`~/.codex/config.toml` if the user wants it to stick.

**Narrow allow rules (optional).** To let only specific commands run outside the
sandbox without opening all network, write
`<project root>/.codex/rules/codex-implement.rules`:

```
prefix_rule(pattern=["npm", "install"], decision="allow", justification="spec step 2")
```

`allow` runs the matching command outside the sandbox with no prompt. Requires the
trust override above. Remove the file after the run unless the user wants to keep it.

## 3. Run codex

Make sure the working tree is clean or that the user knows uncommitted changes
exist, so the review diff is meaningful. Pick a **new run directory**
`<scratchpad>/codex-run-<slug>` for every run, including resumes and retries; the
runner creates it and refuses a directory that already has `events.jsonl`.

Start codex in two parts, so every codex step arrives in this session as a
notification while it works, and codex survives the watcher:

1. Start the runner with a plain **Bash** call (no `run_in_background`); it returns
   immediately with `[STARTED]`:
  ```bash
  ~/.claude/skills/codex-implement/scripts/codex-monitor.sh "<run dir>" \
    --sandbox workspace-write \
    -C "<project root>" \
    -o "<run dir>/last.md" \
    [-m <model>] \
    [-c model_reasoning_effort=<effort>] \
    [-c sandbox_workspace_write.network_access=true] \
    [-c 'projects."<project root>".trust_level="trusted"'] \
    - < "<scratchpad>/codex-spec-<slug>.md"
  ```
2. Follow its progress with the **Monitor** tool:
   - `command`: `~/.claude/skills/codex-implement/scripts/codex-watch.sh "<run dir>"`
   - `description`: `codex: <slug>`
   - `timeout_ms`: `1800000`

The runner detaches itself into a user systemd scope, or a separate process group
when user systemd is unavailable. Codex must not remain a child of anything Claude
Code can kill: Monitor expiry, the background Bash time limit, or low-memory reaping.

Run only the watch under Monitor. Monitor has no `persistent` option; `timeout_ms`
defaults to 5 minutes and is capped at 30, and on expiry Monitor kills its command
with every child.

When the watch expires before `[EXIT]`, arm the same Monitor again. `codex-watch.sh`
remembers how many lines it already printed (`<run dir>/progress.seen`), so a new
watch continues without repeating events. It exits after `[EXIT]`. After 15 seconds
without a new line, it also checks whether the run is alive and reports `[FAILED]`
if the runner disappeared without writing `[EXIT]`.

Replace `--sandbox workspace-write` with `--approve-for-me` when you chose auto
review in step 2; never pass both.

Start the commands with exactly `~/.claude/skills/codex-implement/scripts/codex-monitor.sh`
and `~/.claude/skills/codex-implement/scripts/codex-watch.sh` (or
`~/.claude/skills/codex-implement/scripts/codex-stop.sh` to stop), and never prefix them
with env var assignments. Nix-managed settings allow these commands without a prompt,
and a `FOO=1 …` prefix breaks the match. Runner options go right
after the script name, before the run directory.

`codex-monitor.sh` runs `codex-latest.sh exec --json` with your arguments, saves the
raw events to `<run dir>/events.jsonl` and stderr to `<run dir>/stderr.log`, and
prints only the events that may need your attention, one line each, through `jq`,
to the detached runner's stdout and to `<run dir>/progress.log`, which
`codex-watch.sh` follows.
It also always passes `--add-dir ~/.cache/nix`: nix's eval and fetcher caches are
SQLite files there, and without it every nix command in the sandbox fails with
`attempt to write a readonly database`. So do not tell codex to move the nix cache
or set `XDG_CACHE_HOME`; the warm cache is already usable.
`codex-latest.sh` builds `github:nixos/nixpkgs/nixos-unstable#codex` with nix and
runs that binary; only if
the nix build fails does it fall back to the locally installed `codex`. It logs which
one it used to `stderr.log`; mention that in your report. Pass `--local` before the
run directory to skip nix when the user asks for the local one.

The run directory is the handle for its processes, codex thread, and output:

| File | Contents |
|---|---|
| `events.jsonl` | Raw codex events. |
| `progress.log` | Progress lines consumed by the watch. |
| `stderr.log` | Nix build and codex stderr. |
| `runner.out` | Detached runner stdout and stderr, including startup errors. |
| `stdin` | Saved spec or other input; empty when the launcher's stdin is a terminal. |
| `unit` | Systemd scope name; absent when using a plain process group. |
| `pid` | Runner pid and process group id. |
| `thread` | Codex thread id, recorded as soon as `thread.started` arrives. |
| `last.md` | Codex's final message, written by `-o` in the examples. |
| `progress.seen` | Number of progress lines already printed by the watch. |

**Accounts.** There is more than one codex account, each with its own usage limit:
`codex` (the default, `~/.codex`) and `codex1` (`~/.codex1`). Start with the default
and pass `--account codex1` before the run directory only after the default runs out.
The runner then sets `CODEX_HOME`; it refuses with `[FAILED] no codex account in ...`
when that account has never logged in.

A usage-limit failure looks like `[FAILED]` or a non-zero `[EXIT]` whose reason in
`<run dir>/stderr.log` or `last.md` mentions a usage or rate limit, often with a reset
time. When that happens, rerun the **whole spec** on the next account: sessions live
under `CODEX_HOME`, so another account cannot resume the previous account's run. Say in
your report which account did the work.

Event lines:

| Line | Meaning | What to do |
|---|---|---|
| `[STARTED] <unit.scope or pid N> — follow with codex-watch.sh "<run dir>"` | The detached runner has started. | Arm the Monitor watch. |
| `[FAIL rc=N] <cmd> :: <last output line>` | A command failed. Sent once per distinct command. | Usually nothing; codex fixes its own failures. |
| `[BLOCKED rc=N] <cmd> :: <line>` | The failure looks like a sandbox denial: DNS or network errors, `Operation not permitted`, read-only filesystem. | codex cannot fix this. If the spec needs it, stop and resume with the missing flag. |
| `[STUCK?] same command failed 3 times: <cmd>` | codex keeps retrying one failing command. | Read the raw events; stop it if it is going nowhere. |
| `[QUIET] no codex events for <time>; still running: <cmd>` | Nothing new for `--quiet-secs` seconds (default 600). Sent once per silent stretch. | Normal for a long test suite or the first nix build. A server started in the foreground that never returns is a hang: stop it. |
| `[plan d/n] next: <item>` | codex's todo list advanced. | Nothing. |
| `[FAIL mcp] <server>.<tool>` | An MCP tool call failed. | Usually nothing. |
| `[ERROR] ...` | A non-fatal error, such as a dropped stream codex retries. | Nothing unless it repeats. |
| `[DONE] N cmds (M failed), K files changed, ...` | The turn completed. | Wait for `[EXIT]`. |
| `[FAILED] ...` | The turn failed, or codex exited without finishing one. | Read the reason; see below. |
| `[FAILED] runner is gone without [EXIT]; see ...` | The run died without an exit marker. | Read `runner.out` and `stderr.log`, then retry or resume in a new run directory. |
| `[EXIT] codex rc=N` | Always last; the stream then ends. | Go to step 4. |
| `[EXIT] codex stopped by codex-stop.sh` | The run was stopped. | Resume in a new run directory if needed. |

Not sent: successful commands, codex's messages, per-file edits, and web searches.
Exit code 1 from `rg`, `grep`, `test`, `diff`, or `cmp` counts as "no match" and is
not reported as a failure. Everything is still in `events.jsonl`.

While it runs:

- Act only on `[BLOCKED]`, `[STUCK?]`, a `[QUIET]` that points at a hang, and
  `[FAILED]`. To stop codex, run
  `~/.claude/skills/codex-implement/scripts/codex-stop.sh "<run dir>"`, then resume
  as in step 4. TaskStop on the Monitor only ends the watch.
- Tell the user about those same events and what you decided. Do not relay the rest.
- Do not poll or sleep. Keep doing independent work, or wait for events.
- File edits are not shown live. Check for edits outside the spec's area in step 4.
- If the project's test suite routinely runs longer than ten minutes, pass
  `--quiet-secs <seconds>` before the run directory to raise the threshold.
- After `[EXIT]`, go to step 4. The runner and watch end by themselves.

Other useful flags:

- `--add-dir <dir>` when codex must write outside the project root.
- `--skip-git-repo-check` when the target is not a git repository.

If codex failed (`[FAILED]`, or `[EXIT]` with a non-zero rc), read the reason
first. If it was blocked by the sandbox, do not rewrite the spec: resume the same
session with the missing flag added (see step 4). If it was a usage limit, start the
same spec again with `--account codex1`. Otherwise fix the spec or
environment and run again. Do not silently take over the implementation yourself
on the first failure.

## 4. Review

After codex finishes:

1. Read `<run dir>/last.md` for codex's summary and any caveats it raised.
2. Run `git status` and `git diff` and read the whole diff.
3. Run every acceptance command from the spec yourself.
4. Check the diff against the spec: missing requirements, scope creep, changed files
   outside the allowed area, new dependencies, deleted tests, leftover
   `.codex/rules` files you created.

The progress lines are truncated and skip most events. For the full output of the
commands that failed, read the raw events (use `nix run nixpkgs#jq --` in place of `jq` if it is not
installed):

```bash
jq -r 'select(.type == "item.completed" and .item.type == "command_execution"
  and .item.exit_code != 0) | "$ \(.item.command)\n\(.item.aggregated_output)"' \
  "<run dir>/events.jsonl"
```

For small problems, fix them directly. For larger gaps, send a follow-up to the same
codex session instead of starting over. Start the runner and the watch again as in
step 3, with a new run directory, the same flags as the first run plus any that were
missing. Pass `--resume-from "<previous run dir>"` before the new run directory and
`resume "<prompt>"` at the end of the codex arguments:

```bash
~/.claude/skills/codex-implement/scripts/codex-monitor.sh \
  --resume-from "<previous run dir>" "<new run dir>" \
  --sandbox workspace-write \
  -C "<project root>" \
  -o "<new run dir>/last.md" \
  [-m <model>] \
  [-c model_reasoning_effort=<effort>] \
  [-c sandbox_workspace_write.network_access=true] \
  resume "<what is wrong and what to change>" </dev/null
```

Keep the `</dev/null`: without a spec on stdin, codex would otherwise wait for more
input on it.

The runner reads the previous run's `thread` and inserts that id after `resume`.
Never use `--last`: it picks the most recent session of the whole account, which
can be the wrong session when several runs are in flight. The runner refuses a
resume while the previous run is alive; resumes to one codex session must be serial.

Keep all flags before `resume`. Use `--approve-for-me` instead of `--sandbox` here
too if that is what the first run used. Repeat the user's `-m` and
`model_reasoning_effort` on every resume; do not rely on the resumed session
remembering them.

Then review again.

## 5. Report

Tell the user, in this order: whether the acceptance commands pass, which codex
binary ran (nix unstable or local fallback) and which account, which flags you chose and why (model,
reasoning effort, network, approve-for-me, trust), what codex changed, what you
changed after review, and anything left open. Do not commit unless asked.
