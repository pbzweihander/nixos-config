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

**Model.** Omit `-m` unless the user names a model; the default comes from
`~/.codex/config.toml`. If the user names one, pass `-m <model>`.

**Network.** The workspace-write sandbox blocks outbound network by default. Enable
it up front when the task needs it, and say so in the spec:

- Needs network: installing or updating dependencies (`npm install`, `cargo add`,
  `pip install`, `go get`, `nix build` of new inputs), fetching schemas or fixtures,
  calling an external API, cloning anything.
- No network: pure code edits, running existing tests, builds whose dependencies are
  already vendored or cached.

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
exist, so the review diff is meaningful. Then run codex from the project root with
the spec on stdin:

```bash
~/.claude/skills/codex-implement/scripts/codex-latest.sh exec \
  --sandbox workspace-write \
  -C "<project root>" \
  -o "<scratchpad>/codex-last-<slug>.md" \
  [-m <model>] \
  [-c sandbox_workspace_write.network_access=true] \
  [-c 'projects."<project root>".trust_level="trusted"'] \
  - < "<scratchpad>/codex-spec-<slug>.md"
```

Replace `--sandbox workspace-write` with `--approve-for-me` when you chose auto
review in step 2; never pass both.

`codex-latest.sh` builds `github:nixos/nixpkgs/nixos-unstable#codex` with nix and
runs that binary. Only if the nix build fails does it fall back to the locally
installed `codex`. It logs which one it used on stderr; mention that in your report.
Set `CODEX_LATEST_FORCE_LOCAL=1` to skip nix when the user asks for the local one.

Run it in the background: the first nix build can take a minute and the codex run
itself can take many minutes. Wait for the completion notification. Do not poll.

Other useful flags:

- `--add-dir <dir>` when codex must write outside the project root.
- `--skip-git-repo-check` when the target is not a git repository.

If codex exits non-zero or the last message reports it could not finish, read its
output first. If it was blocked by the sandbox (network refused, permission denied
outside the workspace), do not rewrite the spec: resume the same session with the
missing flag added (see step 4). Otherwise fix the spec or environment and run
again. Do not silently take over the implementation yourself on the first failure.

## 4. Review

After codex finishes:

1. Read `codex-last-<slug>.md` for codex's summary and any caveats it raised.
2. Run `git status` and `git diff` and read the whole diff.
3. Run every acceptance command from the spec yourself.
4. Check the diff against the spec: missing requirements, scope creep, changed files
   outside the allowed area, new dependencies, deleted tests, leftover
   `.codex/rules` files you created.

For small problems, fix them directly. For larger gaps, send a follow-up to the same
codex session instead of starting over, carrying the same flags plus any that were
missing:

```bash
~/.claude/skills/codex-implement/scripts/codex-latest.sh exec \
  --sandbox workspace-write \
  -C "<project root>" \
  [-c sandbox_workspace_write.network_access=true] \
  resume --last "<what is wrong and what to change>"
```

Flags must come before `resume`; `resume` itself only accepts `-c`, `--last`, and
`-i`. Use `--approve-for-me` instead of `--sandbox` here too if that is what the
first run used.

Then review again.

## 5. Report

Tell the user, in this order: whether the acceptance commands pass, which codex
binary ran (nix unstable or local fallback), which flags you chose and why (model,
network, approve-for-me, trust), what codex changed, what you changed after review,
and anything left open. Do not commit unless asked.
