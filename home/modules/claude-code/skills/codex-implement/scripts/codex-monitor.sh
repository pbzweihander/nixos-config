#!/usr/bin/env bash
# Run `codex exec --json` and print one progress line per event, for Claude Code's Monitor tool.
# usage: codex-monitor.sh <run-dir> <codex exec args...>   (stdin is passed through to codex)
# Writes <run-dir>/events.jsonl (raw events) and <run-dir>/stderr.log (nix build + codex stderr).
set -uo pipefail
here="$(dirname "$(readlink -f "$0")")"
codex_latest="${CODEX_LATEST:-$here/codex-latest.sh}"
dir="$1"; shift
mkdir -p "$dir"

jq_cmd=(jq)
command -v jq >/dev/null 2>&1 || jq_cmd=(nix run nixpkgs#jq --)

"$codex_latest" exec --json "$@" 2>"$dir/stderr.log" \
  | tee "$dir/events.jsonl" \
  | "${jq_cmd[@]}" -r --unbuffered -f "$here/codex-progress.jq"
rc=${PIPESTATUS[0]}

if ! grep -qE '"type":"turn\.(completed|failed)"' "$dir/events.jsonl"; then
  why="$(grep -iE -m 3 'error|failed|denied|not found|unauthori' "$dir/stderr.log" | tr '\n' ' ')"
  echo "[FAILED] codex exited without finishing a turn: ${why:-$(tail -n 3 "$dir/stderr.log" | tr '\n' ' ')}"
fi
echo "[EXIT] codex rc=$rc"
