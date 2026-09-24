#!/usr/bin/env bash
# Follow <run-dir>/progress.log written by codex-monitor.sh, for Claude Code's Monitor tool.
# usage: codex-watch.sh <run-dir>
# Prints each progress line once, across re-arms: the count of lines already printed is
# kept in <run-dir>/progress.seen, so a re-armed watch resumes where the expired one
# stopped. Exits after the [EXIT] line.
set -uo pipefail
dir="$1"
log="$dir/progress.log"
seen_file="$dir/progress.seen"
seen=$(cat "$seen_file" 2>/dev/null || echo 0)

exec 3< <(exec tail -n "+$((seen + 1))" -F "$log" 2>/dev/null)
tail_pid=$!
trap 'kill "$tail_pid" 2>/dev/null' EXIT

while IFS= read -r line <&3; do
  printf '%s\n' "$line"
  seen=$((seen + 1))
  echo "$seen" > "$seen_file"
  case "$line" in "[EXIT]"*) exit 0 ;; esac
done
