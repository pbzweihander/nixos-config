#!/usr/bin/env bash
# Build-time smoke test for claude-wiki. usage: smoke-test.sh <claude-wiki executable>
# The SessionStart hook runs claude-wiki in every session, so a broken build must fail
# here rather than in a session.
set -euo pipefail
wiki=$1

HOME=$(mktemp -d)
export HOME CLAUDE_WIKI_DIR=$HOME/wiki
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.com
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.com
P=$CLAUDE_WIKI_DIR/pages

fail() {
  printf 'smoke test failed: %s\n$ %s\n%s\n' "$1" "$2" "$3" >&2
  exit 1
}
check() { # check <what> <pattern> <command...>: the output must contain the pattern
  local what=$1 pattern=$2 out
  shift 2
  out=$("$@" 2>&1) || fail "$what" "$*" "$out"
  grep -q -- "$pattern" <<<"$out" || fail "$what" "$*" "$out"
}
check_not() { # check_not <what> <pattern> <command...>: the output must not contain it
  local what=$1 pattern=$2 out
  shift 2
  out=$("$@" 2>&1) || fail "$what" "$*" "$out"
  ! grep -q -- "$pattern" <<<"$out" || fail "$what" "$*" "$out"
}

check "empty wiki" "no pages" "$wiki" list

cat >"$P/knowledge/nix-sandbox.md" <<'EOF'
---
title: nix commands fail inside the codex sandbox
tags:
  - nix
  - sandbox
project: demo
created: 2026-09-16
---
The sandbox blocked the daemon socket, then nix failed with
`attempt to write a readonly database`.
EOF
cat >"$P/knowledge/replica-notes.md" <<'EOF'
---
title: replica notes
tags: [postgres]
created: 2026-09-16
---
A readonly replica. The database is postgres.
EOF
cat >"$P/projects/demo.md" <<'EOF'
---
title: demo
created: 2026-09-16
---
Demo is a Rust service. Build with `cargo build`.
EOF
for s in open done; do
  printf -- '---\ntitle: %s task\nproject: demo\nstatus: %s\ncreated: 2026-09-16\n---\nbody\n' "$s" "$s" \
    >"$P/followups/$s-task.md"
done
printf -- '---\ntitle: no status\nproject: demo\ncreated: 2026-09-16\n---\nbody\n' >"$P/followups/no-status.md"
printf 'no frontmatter\n' >"$P/knowledge/bare.md"
printf -- '---\ntitle: [unclosed\n---\nbody\n' >"$P/history/2026-09-16-bad.md"

out=$("$wiki" sync 2>&1)
check "sync warns about a page without frontmatter" "bare.md has no frontmatter" echo "$out"
check "sync warns about a follow-up without status" "no-status.md has no status" echo "$out"
check "sync committed everything" "nothing to commit" "$wiki" sync
check "rebuild" "nothing to commit" "$wiki" sync --rebuild

check "porter stemming" "nix-sandbox.md" "$wiki" search blocking
check "block-style YAML tags" "tags=nix,sandbox" "$wiki" search --tag sandbox nix
check "invalid YAML still indexes the body" "2026-09-16-bad.md" "$wiki" search body
# both pages contain both words; only nix-sandbox.md has the exact phrase
check "exact phrase ranks first" "nix-sandbox.md" bash -c '"$1" search "readonly database" -n 1' _ "$wiki"
check "a project page takes its project from the file name" "projects/demo.md" "$wiki" search cargo --project demo
check_not "status filter" "done-task" "$wiki" list --type followups --status open

git init -q "$HOME/demo"
git -C "$HOME/demo" commit -q --allow-empty -m init
git -C "$HOME/demo" worktree add -q "$HOME/demo-worktree"
cd "$HOME/demo"
check "context shows the project overview" "Demo is a Rust service" "$wiki" context
check "context shows open follow-ups" "open-task" "$wiki" context
check_not "context hides done follow-ups" "done-task" "$wiki" context
cd "$HOME/demo-worktree"
check "a worktree resolves to its main repository" "Current project: demo" "$wiki" context
cd /
check "context outside a repository" "Shared wiki: 8 pages" "$wiki" context

# sync with paths commits only those pages and leaves other sessions' edits alone
printf -- '---\ntitle: mine\ncreated: 2026-09-16\n---\nmine\n' >"$P/knowledge/mine.md"
printf -- '---\ntitle: theirs\ncreated: 2026-09-16\n---\ntheirs\n' >"$P/knowledge/theirs.md"
out=$("$wiki" sync "$P/knowledge/mine.md" 2>&1)
check "sync with an absolute path commits that page" "committed: wiki: add knowledge/mine" echo "$out"
check "sync with a path reports what it left" "left uncommitted.*theirs.md" echo "$out"
check "other sessions' pages stay uncommitted" "knowledge/theirs.md" git -C "$CLAUDE_WIKI_DIR" status --porcelain
check_not "the commit excludes other pages" "theirs" git -C "$CLAUDE_WIKI_DIR" show --stat HEAD
rm "$P/knowledge/mine.md"
check "sync with a root-relative path commits a deletion" "delete knowledge/mine" \
  "$wiki" sync pages/knowledge/mine.md
check "sync without paths commits the rest" "add knowledge/theirs" "$wiki" sync
echo "claude-wiki smoke test passed"
