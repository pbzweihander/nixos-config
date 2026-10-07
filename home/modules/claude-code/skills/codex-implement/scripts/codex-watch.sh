#!/usr/bin/env bash
# Follow <run-dir>/progress.log written by codex-monitor.sh, for a Claude Code watcher.
# usage: codex-watch.sh [--wait SECS] <run-dir>
# Drops routine [plan ...], [FAIL rc=...], and [FAIL mcp] lines; prints everything else.
# progress.seen counts all consumed lines, including dropped ones, across re-arms.
# Normally exits after [EXIT]. --wait SECS (a positive integer) also returns at the
# first [BLOCKED...], [STUCK?], [QUIET], or [FAILED] line, or prints [WAITING] when
# time runs out. A wait on an already consumed [EXIT] prints that line again.
set -uo pipefail
# Preserve the fractional start time so a one-second wait cannot expire early.
started=${EPOCHREALTIME/./}
here="$(dirname "$(readlink -f "$0")")"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=codex-lib.sh
source "$here/codex-lib.sh"
usage() { printf 'usage: codex-watch.sh [--wait SECS] <run-dir> (SECS must be a positive integer)\n' >&2; exit 2; }
wait_secs=""
if [ "${1:-}" = --wait ]; then
  [ $# -ge 3 ] && [[ "$2" =~ ^[1-9][0-9]*$ ]] || usage
  wait_secs="$2"
  shift 2
fi
[ $# -eq 1 ] || usage
dir="$1"
log="$dir/progress.log"
seen_file="$dir/progress.seen"
seen=$(cat "$seen_file" 2>/dev/null || echo 0)
dead_secs="${CODEX_WATCH_DEAD_SECS:-15}"

# A Monitor can be re-armed even after a previous watch already printed [EXIT].
if exit_line=$(head -n "$seen" "$log" 2>/dev/null | grep '^\[EXIT\]'); then
  if [ -n "$wait_secs" ]; then printf '%s\n' "$exit_line"; fi
  exit 0
fi

exec 3< <(exec tail -n "+$((seen + 1))" -F -s 0.2 "$log" 2>/dev/null)
tail_pid=$!
trap 'kill "$tail_pid" 2>/dev/null; wait "$tail_pid" 2>/dev/null' EXIT

print_line() {
  seen=$((seen + 1))
  echo "$seen" > "$seen_file"
  case "$line" in "[plan "*|"[FAIL rc="*|"[FAIL mcp]"*) return ;; esac
  printf '%s\n' "$line"
  case "$line" in "[EXIT]"*) exit 0 ;; esac
}

while :; do
  if IFS= read -r -t "$dead_secs" line <&3; then
    print_line
    if [ -n "$wait_secs" ]; then
      case "$line" in "[BLOCKED"*|"[STUCK?]"*|"[QUIET]"*|"[FAILED]"*) exit 0 ;; esac
    fi
  elif ! codex_run_alive "$dir"; then
    # tail may not have forwarded the final writes yet. Drain the file itself
    # before deciding that the runner disappeared without its exit marker.
    while IFS= read -r line; do print_line; done < <(tail -n "+$((seen + 1))" "$log" 2>/dev/null)
    printf '[FAILED] runner is gone without [EXIT]; see %s/runner.out and %s/stderr.log\n' "$dir" "$dir"
    exit 1
  fi
  # Check after every read, so even a busy stream of dropped lines stays bounded.
  if [ -n "$wait_secs" ] && [ "$(( (${EPOCHREALTIME/./} - started) / 1000000 ))" -ge "$wait_secs" ]; then
    printf '[WAITING] codex is still running; run the watch again\n'
    exit 0
  fi
done
