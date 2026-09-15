---
name: claude-wiki
description: Search and write the shared markdown wiki at ~/.local/share/claude-wiki that all Claude Code sessions use to share knowledge, project overviews, follow-ups, and work history. Use before non-trivial work to find what earlier sessions learned, and when recording a solved problem, an environment quirk, a project overview, leftover work, or a history entry. Also when the user says "위키에 기록해", "위키 찾아봐".
---

> **This skill is managed by Nix.** `~/.claude/skills/claude-wiki` is a read-only
> symlink into the Nix store. To change this skill, the `claude-wiki` CLI, or the
> wiki rule in `~/.claude/rules/claude-wiki.md`, edit the source at
> `~/nixos-config/home/modules/claude-code/` and run `home-manager switch` (or
> `nixos-rebuild switch`) to apply. Never try to edit the files under
> `~/.claude/skills/` or `~/.claude/rules/` directly. The wiki pages themselves are
> ordinary files; edit them freely.

# Claude wiki

```
~/.local/share/claude-wiki/              git repository
  pages/knowledge/<topic>.md             how things work, fail, and get fixed
  pages/projects/<project>.md            one overview per project
  pages/followups/<slug>.md              work left for a later session
  pages/history/<YYYY-MM-DD>-<slug>.md   what one task did
  index.sqlite                           derived BM25 index, git-ignored
```

The markdown pages are the source of truth. Every `claude-wiki` command first
re-indexes pages whose mtime changed, so pages edited with Edit or Write are
searchable right away. A SessionStart hook runs `claude-wiki context`, which prints
the current project's name and overview page, its open follow-ups and knowledge
pages, recently updated pages, and the tags in use.

**The wiki is English only**: page titles, bodies, tags, and search keywords, even
when the conversation is in Korean or another language.

## Page types

- **knowledge**: a problem's symptom, cause, and fix; a tool or environment quirk;
  how something works; the reason behind a decision. One topic per page, edited in
  place whenever it changes. Include the **exact error message** or identifiers so a
  search on them hits, how you confirmed the cause, the commands of the fix, and the
  conditions under which it applies (versions, machines).
- **projects**: `<project>.md`, what a new session needs in its first minute: what
  the project is, stack, layout, how to build, test, run, and deploy it, related
  repositories and services, and gotchas. Its first 60 lines are printed at every
  session start in that project, so keep it short, and correct it as soon as
  something in it stops being true. Write one after you have explored a project
  substantially and none exists.
- **followups**: work noticed during a task that is out of scope but should be done
  later, including conditional work ("remove this workaround once X is released").
  Record one instead of only mentioning leftover work in your final reply. Set
  `status: open`. Open follow-ups of the current project are printed at session
  start; when one is done, set `status: done` and add a line saying how and when.
- **history**: after a substantial task (not a quick question), 5 to 15 lines: the
  goal, what was done and where (repository, main files), the outcome, and links to
  the knowledge pages and follow-ups it produced. A log: add entries, never rewrite
  old ones.

**Not for the wiki:** rules that must apply every time, such as coding style, commit
conventions, or required workflows. The wiki is only read when searched. Those rules
belong in CLAUDE.md or in `~/.claude/rules/` (a rule file with `paths:` frontmatter
loads only when matching files are touched, for example `**/*.tf`); suggest that to
the user instead. Also never record secrets, credentials, tokens, personal data, or
what the repository or its docs already make obvious.

## Search

```bash
claude-wiki search <terms...> [-n 10] [--type TYPE] [--status open|done] [--project NAME] [--tag TAG]
claude-wiki list [-n 20] [--type ...] [--status ...] [--project ...] [--tag ...]   # recently updated
claude-wiki tags                                                                  # tags and projects in use
```

- Terms are stemmed (Porter) and prefix-matched, and the terms are OR-ed: `configure`
  also finds `configuration` and `configured`. Pass several keywords and synonyms
  rather than one exact phrase. Ranking is BM25, weighted title > tags > project >
  body.
- Quote a multi-word argument to also match it as an exact phrase; pages containing
  the phrase rank higher: `claude-wiki search "readonly database" nix sandbox`.
- Translate the user's words into English keywords before searching. Exact error
  messages and identifiers make good terms.
- Each hit prints the page's absolute path, title, type, status, update date,
  project, tags, and a snippet with matches in `[brackets]`. Read the page with the
  Read tool before relying on it.

## Write

1. **Search first.** If a page on the topic exists, update it with Edit instead of
   creating a near-duplicate. When it is wrong or stale, fix it in place rather than
   appending a contradiction.
2. **Otherwise create a page** with Write at
   `~/.local/share/claude-wiki/pages/<type>/<name>.md`:

   ```markdown
   ---
   title: nix commands fail inside the codex sandbox
   tags: [nix, codex, sandbox]
   project: nixos-config
   created: 2026-09-15
   ---

   Body.
   ```

   - name: short lowercase kebab-case naming the topic; the project name for
     `projects`; `<YYYY-MM-DD>-<slug>` for `history`.
   - `tags`: a few lowercase words. Run `claude-wiki tags` and reuse existing tags.
   - `project`: the current project as the session start summary prints it
     (`Current project:`), which is the main checkout's directory name even inside a
     worktree. Omit it for knowledge not tied to one project. Project pages take it
     from their file name.
   - `status`: `open` or `done`, follow-ups only.
   - `created`: today's date. The update date comes from the file mtime.
3. **Run `claude-wiki sync -m "<short summary>" <pages>`** after every write, passing
   every page you created, edited, or deleted (absolute paths, or relative to the wiki
   root). It indexes the pages and commits only those, so other sessions' unfinished
   edits stay out of your commit. It warns about pages outside `pages/<type>/` and
   about frontmatter that is missing, is not valid YAML, or lacks a required field;
   fix those pages. Plain `claude-wiki sync` commits every change; it is only needed
   to pick up edits left behind by a session that ended without syncing.

Write in English; quote commands, identifiers, and error messages verbatim, even ones
in another language. Make every page readable on its own.

## Maintenance

- `claude-wiki sync --rebuild` rebuilds the index from scratch.
- Deleting or renaming a page is a normal file operation followed by
  `claude-wiki sync <old and new paths>`.
- History of the wiki itself: `git -C ~/.local/share/claude-wiki log`.
