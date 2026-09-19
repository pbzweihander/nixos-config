#!/usr/bin/env bash
# Run `codex exec --json` and print only important progress lines, for Claude Code's Monitor tool.
# usage: codex-monitor.sh [--account NAME] [--local] [--quiet-secs N] <run-dir> <codex exec args...>
#   (stdin is passed through to codex)
# Writes <run-dir>/events.jsonl (raw events) and <run-dir>/stderr.log (nix build + codex stderr).
# Prints [QUIET] when codex has emitted nothing for --quiet-secs seconds (default 600).
# --local skips the nixpkgs-unstable build and runs the locally installed codex.
# --account runs as another codex account: NAME is the directory ~/.<NAME> used as
# CODEX_HOME (login, sessions and config live there), so `codex1` means ~/.codex1.
set -uo pipefail
here="$(dirname "$(readlink -f "$0")")"
codex_latest="${CODEX_LATEST:-$here/codex-latest.sh}"
quiet_secs="${CODEX_QUIET_SECS:-600}"
quiet_poll="${CODEX_QUIET_POLL:-60}"
# Options are flags rather than env var prefixes: a `FOO=1 cmd` prefix keeps the command
# from matching Claude Code's Bash allow rule for this script.
while [ $# -gt 0 ]; do
  case "$1" in
    --quiet-secs) quiet_secs="$2"; shift 2 ;;
    --local) export CODEX_LATEST_FORCE_LOCAL=1; shift ;;
    --account)
      if [ ! -f "$HOME/.$2/auth.json" ]; then
        echo "[FAILED] no codex account in $HOME/.$2; run 'CODEX_HOME=$HOME/.$2 codex login' first" >&2
        exit 2
      fi
      export CODEX_HOME="$HOME/.$2"; shift 2 ;;
    *) break ;;
  esac
done
dir="$1"; shift
mkdir -p "$dir"
: > "$dir/events.jsonl"

jq_cmd=(jq)
command -v jq >/dev/null 2>&1 || jq_cmd=(nix run nixpkgs#jq --)

watch_quiet() {
  local warned="" m idle last
  while sleep "$quiet_poll" >/dev/null 2>&1; do
    m=$(stat -c %Y "$dir/events.jsonl") || continue
    idle=$(( $(date +%s) - m ))
    if [ "$idle" -ge "$quiet_secs" ] && [ "$warned" != "$m" ]; then
      last=$(tail -n 1 "$dir/events.jsonl" | "${jq_cmd[@]}" -r \
        'if .type == "item.started" and .item.type == "command_execution" then "still running: " + (.item.command | sub("^\\S*bash -lc "; "")) else "last event: " + .type end' \
        2>/dev/null | cut -c1-160)
      if [ "$idle" -ge 120 ]; then t="$(( idle / 60 )) min"; else t="${idle}s"; fi
      echo "[QUIET] no codex events for $t; ${last:-no events yet (nix build or startup)}"
      warned=$m
    fi
  done
}
watch_quiet &
watcher=$!

# nix keeps its eval/fetcher caches as SQLite files under ~/.cache/nix; the sandbox
# makes them read-only, and every nix command then fails with "attempt to write a
# readonly database". Keep them writable so codex reuses the warm cache.
nix_cache="${XDG_CACHE_HOME:-$HOME/.cache}/nix"
mkdir -p "$nix_cache"

"$codex_latest" exec --json --add-dir "$nix_cache" "$@" 2>"$dir/stderr.log" \
  | tee "$dir/events.jsonl" \
  | "${jq_cmd[@]}" -n -r --unbuffered -f "$here/codex-progress.jq"
rc=${PIPESTATUS[0]}
kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null

if ! grep -qE '"type":"turn\.(completed|failed)"' "$dir/events.jsonl"; then
  why="$(grep -iE -m 3 'error|failed|denied|not found|unauthori' "$dir/stderr.log" | tr '\n' ' ')"
  echo "[FAILED] codex exited without finishing a turn: ${why:-$(tail -n 3 "$dir/stderr.log" | tr '\n' ' ')}"
fi
echo "[EXIT] codex rc=$rc"
