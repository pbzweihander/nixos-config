#!/usr/bin/env bash
# Detach `codex exec --json` from Claude Code's process tree and follow progress with
# codex-watch.sh under Monitor. Each invocation owns a new run directory.
# usage: codex-monitor.sh [--account NAME] [--local] [--quiet-secs N] [--resume-from PREV_RUN_DIR] <run-dir> <codex exec args...>
#   (stdin is saved before detaching, then passed through to codex)
# Writes <run-dir>/events.jsonl (raw events), <run-dir>/stderr.log (nix build + codex stderr)
# and <run-dir>/progress.log (a copy of the progress lines on stdout).
# Prints [QUIET] when codex has emitted nothing for --quiet-secs seconds (default 600).
# --local skips the nixpkgs-unstable build and runs the locally installed codex.
# --account runs as another codex account: NAME is the directory ~/.<NAME> used as
# CODEX_HOME (login, sessions and config live there), so `codex1` means ~/.codex1.
set -uo pipefail
here="$(dirname "$(readlink -f "$0")")"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=codex-lib.sh
source "$here/codex-lib.sh"
codex_latest="${CODEX_LATEST:-$here/codex-latest.sh}"
quiet_secs="${CODEX_QUIET_SECS:-600}"
quiet_poll="${CODEX_QUIET_POLL:-60}"
foreground=0
resume_from=""
runner_options=()

fail() { printf '[FAILED] %s\n' "$*"; exit 1; }

# Options are flags rather than env var prefixes: a `FOO=1 cmd` prefix keeps the command
# from matching Claude Code's Bash allow rule for this script.
while [ $# -gt 0 ]; do
  case "$1" in
    --foreground) foreground=1; shift ;;
    --quiet-secs)
      [ $# -ge 2 ] || fail "--quiet-secs needs a value"
      quiet_secs="$2"; runner_options+=("$1" "$2"); shift 2 ;;
    --local) export CODEX_LATEST_FORCE_LOCAL=1; runner_options+=("$1"); shift ;;
    --account)
      [ $# -ge 2 ] || fail "--account needs a name"
      if [ ! -f "$HOME/.$2/auth.json" ]; then
        fail "no codex account in $HOME/.$2; run 'CODEX_HOME=$HOME/.$2 codex login' first"
      fi
      export CODEX_HOME="$HOME/.$2"; runner_options+=("$1" "$2"); shift 2 ;;
    --resume-from)
      [ $# -ge 2 ] && [ -n "$2" ] || fail "--resume-from needs a run directory"
      resume_from="$2"; shift 2 ;;
    *) break ;;
  esac
done
[ $# -ge 1 ] || fail "usage: codex-monitor.sh [options] <run-dir> <codex exec args...>"
dir="$1"; shift

if [ "$foreground" -eq 0 ]; then
  [ ! -e "$dir/events.jsonl" ] || fail "run directory already used: $dir; pick a new one"
  mkdir -p -- "$dir" 2>/dev/null || fail "cannot create run directory: $dir"
  # Claim the directory exclusively, including when two launchers race to use it.
  if ! (set -C; : > "$dir/events.jsonl") 2>/dev/null; then
    [ ! -e "$dir/events.jsonl" ] || fail "run directory already used: $dir; pick a new one"
    fail "cannot create $dir/events.jsonl"
  fi
  { : > "$dir/progress.log"; } 2>/dev/null || fail "cannot create $dir/progress.log"

  if [ -n "$resume_from" ]; then
    thread=$(cat "$resume_from/thread" 2>/dev/null) || thread=""
    [ -n "$thread" ] || fail "no codex thread recorded in $resume_from"
    # A run that logged [EXIT] has released its codex thread, even if a straggling
    # child still keeps its scope active.
    if ! grep -q '^\[EXIT\]' "$resume_from/progress.log" 2>/dev/null && codex_run_alive "$resume_from"; then
      fail "previous run $resume_from is still running; resumes to one codex session are serial"
    fi
    args=(); found_resume=0
    for arg in "$@"; do
      [ "$arg" != "--last" ] || fail "--resume-from cannot be used with --last"
      args+=("$arg")
      if [ "$arg" = "resume" ] && [ "$found_resume" -eq 0 ]; then
        args+=("$thread"); found_resume=1
      fi
    done
    [ "$found_resume" -eq 1 ] || fail "--resume-from requires a resume argument"
    set -- "${args[@]}"
  fi

  if [ -t 0 ]; then
    { : > "$dir/stdin"; } 2>/dev/null || fail "cannot save stdin in $dir/stdin"
  else
    cat > "$dir/stdin" 2>/dev/null || fail "cannot save stdin in $dir/stdin"
  fi

  launch=(setsid -f)
  unit=""
  if [ "${CODEX_MONITOR_NO_SYSTEMD:-0}" != 1 ] && systemctl --user show-environment >/dev/null 2>&1; then
    absolute_dir=$(readlink -f -- "$dir") || fail "cannot resolve run directory: $dir"
    basename=$(printf '%s' "${absolute_dir##*/}" | LC_ALL=C tr -c 'A-Za-z0-9_.-' '_')
    hash=$(printf '%s' "$absolute_dir" | sha256sum); hash=${hash:0:8}
    unit="codex-$basename-$hash.scope"
    printf '%s\n' "$unit" > "$dir/unit" || fail "cannot write $dir/unit"
    launch+=(systemd-run --user --scope --quiet --collect "--unit=${unit%.scope}" --)
  fi
  if ! "${launch[@]}" "$here/codex-monitor.sh" --foreground "${runner_options[@]}" "$dir" "$@" \
    < "$dir/stdin" > "$dir/runner.out" 2>&1; then
    fail "runner did not start: $(tail -n 3 "$dir/runner.out" | tr '\n' ' ')"
  fi
  for ((poll = 0; poll < 50; poll++)); do
    if [ -s "$dir/pid" ]; then
      printf '[STARTED] %s — follow with codex-watch.sh "%s"\n' "${unit:-pid $(cat "$dir/pid")}" "$dir"
      exit 0
    fi
    sleep 0.1
  done
  fail "runner did not start: $(tail -n 3 "$dir/runner.out" | tr '\n' ' ')"
fi

printf '%s\n' "$$" > "$dir/pid"
exec > >(tee -a "$dir/progress.log")

jq_cmd=(jq)
command -v jq >/dev/null 2>&1 || jq_cmd=(nix run nixpkgs#jq --)

watch_quiet() {
  local warned="" sleeper="" m idle last
  # An orphaned sleep would keep the scope active after codex finishes. Waiting
  # on it in the background lets TERM interrupt the wait and reap it promptly.
  trap 'if [ -n "$sleeper" ]; then kill "$sleeper" 2>/dev/null; wait "$sleeper" 2>/dev/null; fi; exit 0' TERM INT
  while :; do
    sleep "$quiet_poll" >/dev/null 2>&1 &
    sleeper=$!
    wait "$sleeper" || break
    sleeper=""
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

# Record the thread before forwarding its event, so it is available during the run.
# Keep the progress filter unchanged and append to the files claimed by the launcher.
record_thread() {
  local event thread
  while IFS= read -r event; do
    if [ ! -s "$dir/thread" ]; then
      thread=$(printf '%s\n' "$event" | "${jq_cmd[@]}" -r 'select(.type == "thread.started") | .thread_id // empty')
      if [ -n "$thread" ]; then printf '%s\n' "$thread" > "$dir/thread"; fi
    fi
    printf '%s\n' "$event"
  done
}

"$codex_latest" exec --json --add-dir "$nix_cache" "$@" 2>"$dir/stderr.log" \
  | tee -a "$dir/events.jsonl" \
  | record_thread \
  | "${jq_cmd[@]}" -n -r --unbuffered -f "$here/codex-progress.jq"
rc=${PIPESTATUS[0]}
kill "$watcher" 2>/dev/null; wait "$watcher" 2>/dev/null

if ! grep -qE '"type":"turn\.(completed|failed)"' "$dir/events.jsonl"; then
  why="$(grep -iE -m 3 'error|failed|denied|not found|unauthori' "$dir/stderr.log" | tr '\n' ' ')"
  echo "[FAILED] codex exited without finishing a turn: ${why:-$(tail -n 3 "$dir/stderr.log" | tr '\n' ' ')}"
fi
echo "[EXIT] codex rc=$rc"
