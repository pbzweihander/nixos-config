#!/usr/bin/env bash
# Follow <run-dir>/progress.log written by codex-monitor.sh, for Claude Code's Monitor tool.
# usage: codex-watch.sh <run-dir>
# Prints each progress line once, across re-arms: the count of lines already printed is
# kept in <run-dir>/progress.seen, so a re-armed watch resumes where the expired one
# stopped. Exits after the [EXIT] line.
set -uo pipefail
here="$(dirname "$(readlink -f "$0")")"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=codex-lib.sh
source "$here/codex-lib.sh"
dir="$1"
log="$dir/progress.log"
seen_file="$dir/progress.seen"
seen=$(cat "$seen_file" 2>/dev/null || echo 0)
dead_secs="${CODEX_WATCH_DEAD_SECS:-15}"

# A Monitor can be re-armed even after a previous watch already printed [EXIT].
if head -n "$seen" "$log" 2>/dev/null | grep '^\[EXIT\]' >/dev/null; then exit 0; fi

exec 3< <(exec tail -n "+$((seen + 1))" -F -s 0.2 "$log" 2>/dev/null)
tail_pid=$!
trap 'kill "$tail_pid" 2>/dev/null; wait "$tail_pid" 2>/dev/null' EXIT

print_line() {
  printf '%s\n' "$line"
  seen=$((seen + 1))
  echo "$seen" > "$seen_file"
  case "$line" in "[EXIT]"*) exit 0 ;; esac
}

while :; do
  if IFS= read -r -t "$dead_secs" line <&3; then
    print_line
  elif ! codex_run_alive "$dir"; then
    # tail may not have forwarded the final writes yet. Drain the file itself
    # before deciding that the runner disappeared without its exit marker.
    while IFS= read -r line; do print_line; done < <(tail -n "+$((seen + 1))" "$log" 2>/dev/null)
    printf '[FAILED] runner is gone without [EXIT]; see %s/runner.out and %s/stderr.log\n' "$dir" "$dir"
    exit 1
  fi
done
