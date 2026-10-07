#!/usr/bin/env bash
# Integration tests use real detached processes and, when available, real user scopes.
# No codex account or network is needed; CODEX_LATEST supplies a fake event stream.
set -uo pipefail
here="$(dirname "$(readlink -f "$0")")"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=codex-lib.sh
source "$here/codex-lib.sh"
work=$(mktemp -d)
mode=setup
case_name=setup
watch_pid=""
monitor="$here/codex-monitor.sh"
watch="$here/codex-watch.sh"
stop="$here/codex-stop.sh"

cleanup() {
  if [ -n "$watch_pid" ]; then
    kill "$watch_pid" 2>/dev/null
    wait "$watch_pid" 2>/dev/null
  fi
  local dir pid unit
  for dir in "$work"/pid/* "$work"/systemd/*; do
    if [ -f "$dir/unit" ]; then
      unit=$(cat "$dir/unit")
      systemctl --user stop "$unit" >/dev/null 2>&1
    elif [ -f "$dir/pid" ]; then
      pid=$(cat "$dir/pid")
      kill -KILL -- "-$pid" 2>/dev/null
    fi
  done
  rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

die() { printf 'FAIL [%s] %s: %s\n' "$mode" "$case_name" "$*" >&2; exit 1; }
pass() { printf 'PASS [%s] %s\n' "$mode" "$case_name"; }

poll() {
  local attempt
  for ((attempt = 0; attempt < 100; attempt++)); do
    if "$@"; then return 0; fi
    sleep 0.05
  done
  die "timed out waiting for $*"
}
finished() { grep -q '^\[EXIT\]' "$1/progress.log" 2>/dev/null; }
not_alive() { ! codex_run_alive "$1"; }
group_gone() { ! kill -0 -- "-$(cat "$1/pid")" 2>/dev/null; }

new_run() {
  case_name="$1"
  run="$work/$mode/$1"
  export FAKE_CODEX_DIR="$run"
  unset FAKE_CODEX_SLEEP FAKE_CODEX_RC FAKE_CODEX_PROGRESS
}

launch() {
  # A foreground implementation would hit this timeout while the fake sleeps.
  if ! timeout 7 "$monitor" "$@" > "$work/launch.out" 2> "$work/launch.err"; then
    die "launcher failed: $(cat "$work/launch.out" "$work/launch.err")"
  fi
  grep -q '^\[STARTED\]' "$work/launch.out" || die "missing [STARTED]"
  [ -s "$run/pid" ] || die "missing pid"
  if [ "$mode" = systemd ]; then
    local hash base unit
    hash=$(printf '%s' "$(readlink -f "$run")" | sha256sum); hash=${hash:0:8}
    base=$(printf '%s' "${run##*/}" | LC_ALL=C tr -c 'A-Za-z0-9_.-' '_')
    unit="codex-$base-$hash.scope"
    [ "$(cat "$run/unit")" = "$unit" ] || die "incorrect scope name"
    grep -Fq "[STARTED] $unit" "$work/launch.out" || die "missing scope in [STARTED]"
  else
    [ ! -e "$run/unit" ] || die "unexpected unit in pid mode"
    grep -Fq "[STARTED] pid $(cat "$run/pid")" "$work/launch.out" || die "missing pid in [STARTED]"
  fi
}

refuse() {
  local reason="$1"; shift
  if timeout 7 "$monitor" "$@" > "$work/rejected.out" 2> "$work/rejected.err"; then
    die "launcher accepted invalid invocation"
  fi
  [ "$(wc -l < "$work/rejected.out")" -eq 1 ] || die "expected one failure line on stdout"
  grep -Fq "[FAILED] $reason" "$work/rejected.out" || die "wrong refusal: $(cat "$work/rejected.out")"
}

cat > "$work/fake-codex" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
printf '%s\n' "$@" > "$FAKE_CODEX_DIR/args.received"
while [ $# -gt 0 ]; do
  if [ "$1" = resume ]; then
    printf '%s\n' "${2:-}" > "$FAKE_CODEX_DIR/resume.received"
    printf '%s\n' "${3:-}" > "$FAKE_CODEX_DIR/prompt.received"
    break
  fi
  shift
done
cat > "$FAKE_CODEX_DIR/stdin.received"
printf '%s\n' '{"type":"thread.started","thread_id":"12345678-1234-1234-1234-123456789abc"}'
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"error","message":"fake progress"}'
if [ "${FAKE_CODEX_PROGRESS:-0}" = 1 ]; then
  cat <<'EVENTS'
{"type":"item.updated","item":{"type":"todo_list","items":[{"text":"fake task","completed":false}]}}
{"type":"item.completed","item":{"type":"command_execution","command":"false","exit_code":1}}
{"type":"item.completed","item":{"type":"mcp_tool_call","server":"fake","tool":"test","status":"failed"}}
{"type":"item.completed","item":{"type":"command_execution","command":"first blocked command","exit_code":1,"aggregated_output":"Permission denied"}}
{"type":"item.updated","item":{"type":"todo_list","items":[{"text":"fake task","completed":true}]}}
{"type":"error","message":"after first block"}
{"type":"item.completed","item":{"type":"command_execution","command":"second blocked command","exit_code":1,"aggregated_output":"Permission denied"}}
EVENTS
fi
sleep "${FAKE_CODEX_SLEEP:-0}"
printf '%s\n' '{"type":"item.completed","item":{"type":"command_execution","command":"true","exit_code":0}}'
if [ "${FAKE_CODEX_RC:-0}" -eq 0 ]; then
  printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":2}}'
else
  printf '%s\n' '{"type":"turn.failed","error":{"message":"chosen failure"}}'
fi
exit "${FAKE_CODEX_RC:-0}"
FAKE
chmod +x "$work/fake-codex"
export CODEX_LATEST="$work/fake-codex"
export CODEX_WATCH_DEAD_SECS=0.5
export CODEX_QUIET_POLL=60

modes=(pid)
if systemctl --user show-environment >/dev/null 2>&1; then
  modes+=(systemd)
else
  printf 'SKIP [systemd] user systemd is unreachable\n'
fi

for mode in "${modes[@]}"; do
  if [ "$mode" = pid ]; then export CODEX_MONITOR_NO_SYSTEMD=1; else export CODEX_MONITOR_NO_SYSTEMD=0; fi

  new_run 'normal run + stdin'
  printf 'the spec\nsecond line\n' > "$work/spec"
  launch --local --quiet-secs 20 "$run" - < "$work/spec"
  poll finished "$run"
  poll not_alive "$run"
  poll group_gone "$run"
  grep -q '^\[EXIT\] codex rc=0$' "$run/progress.log" || die "wrong exit marker"
  [ "$(cat "$run/thread")" = 12345678-1234-1234-1234-123456789abc ] || die "missing thread id"
  [ "$(cat "$run/stdin.received")" = "$(cat "$work/spec")" ] || die "stdin was not delivered"
  [ "$(cat "$run/stdin")" = "$(cat "$work/spec")" ] || die "stdin was not saved"
  previous="$run"
  pass

  case_name='watch after completion'
  timeout 5 "$watch" "$run" > "$work/watch.out" || die "late watch failed"
  grep -q '^\[EXIT\] codex rc=0$' "$work/watch.out" || die "late watch missed [EXIT]"
  [ "$(cat "$run/progress.seen")" -eq "$(wc -l < "$run/progress.log")" ] || die "wrong progress.seen"
  timeout 5 "$watch" "$run" > "$work/watch.out" || die "completed watch could not re-arm"
  [ ! -s "$work/watch.out" ] || die "completed watch repeated lines"
  pass

  case_name='wait after completion repeats exit'
  timeout 5 "$watch" --wait 10 "$run" > "$work/watch.out" || die "completed wait failed"
  [ "$(cat "$work/watch.out")" = '[EXIT] codex rc=0' ] || die "completed wait did not repeat only [EXIT]"
  [ "$(cat "$run/progress.seen")" -eq "$(wc -l < "$run/progress.log")" ] || die "completed wait changed progress.seen"
  pass

  case_name='wait rejects invalid seconds'
  for secs in 0 x; do
    rc=0
    timeout 5 "$watch" --wait "$secs" "$run" > "$work/watch.out" 2> "$work/watch.err" || rc=$?
    [ "$rc" -eq 2 ] || die "--wait $secs exited $rc instead of 2"
    grep -q '^usage:' "$work/watch.err" || die "--wait $secs did not print usage on stderr"
    [ ! -s "$work/watch.out" ] || die "--wait $secs printed to stdout"
  done
  pass

  case_name='reuse refused'
  before=$(sha256sum "$run/events.jsonl" "$run/progress.log")
  refuse "run directory already used: $run; pick a new one" "$run" </dev/null
  [ "$(sha256sum "$run/events.jsonl" "$run/progress.log")" = "$before" ] || die "reuse changed the logs"
  pass

  new_run 'watch drops routine lines but counts them'
  export FAKE_CODEX_PROGRESS=1 FAKE_CODEX_RC=7
  launch "$run" </dev/null
  poll finished "$run"
  timeout 5 "$watch" "$run" > "$work/watch.out" || die "filtered watch failed"
  if grep -qE '^\[(plan |FAIL rc=|FAIL mcp\])' "$work/watch.out"; then die "routine lines were printed"; fi
  [ "$(grep -c '^\[BLOCKED' "$work/watch.out")" -eq 2 ] || die "watch dropped [BLOCKED]"
  grep -q '^\[FAILED\] chosen failure$' "$work/watch.out" || die "watch dropped [FAILED]"
  [ "$(wc -l < "$work/watch.out")" -eq 6 ] || die "watch lost other progress lines"
  [ "$(cat "$run/progress.seen")" -eq 10 ] || die "watch did not count dropped lines"
  [ "$(cat "$run/progress.seen")" -eq "$(wc -l < "$run/progress.log")" ] || die "watch skipped progress"
  pass

  new_run 'wait returns at a block while alive'
  export FAKE_CODEX_PROGRESS=1 FAKE_CODEX_SLEEP=30
  launch "$run" </dev/null
  # Buffer both decisions before watching to catch consuming past the first one.
  poll grep -q '^\[BLOCKED rc=1\] second blocked command' "$run/progress.log"
  timeout 5 "$watch" --wait 10 "$run" > "$work/watch.out" || die "wait did not return at first block"
  codex_run_alive "$run" || die "wait only returned after the run stopped"
  grep -q '^\[BLOCKED rc=1\] first blocked command' "$work/watch.out" || die "wait missed first block"
  [ "$(wc -l < "$work/watch.out")" -eq 2 ] || die "wait printed routine lines or consumed past the block"
  [ "$(cat "$run/progress.seen")" -eq 5 ] || die "wait did not count dropped lines"
  pass

  case_name='wait re-arm continues after the block'
  timeout 5 "$watch" --wait 10 "$run" > "$work/watch.out" || die "wait did not return at second block"
  codex_run_alive "$run" || die "second wait only returned after the run stopped"
  grep -q '^\[ERROR\] after first block$' "$work/watch.out" || die "wait lost progress after first block"
  grep -q '^\[BLOCKED rc=1\] second blocked command' "$work/watch.out" || die "wait missed second block"
  [ "$(wc -l < "$work/watch.out")" -eq 2 ] || die "wait repeated progress or printed routine lines"
  [ "$(cat "$run/progress.seen")" -eq 8 ] || die "second wait saved the wrong position"
  pass

  case_name='wait expires on a sleeping run'
  printf '%s\n' '[STARTED] runner message' '[DONE] turn complete' '[NOTICE] unknown runner message' >> "$run/progress.log"
  started=${EPOCHREALTIME/./}
  timeout 5 "$watch" --wait 1 "$run" > "$work/watch.out" || die "bounded wait did not exit 0"
  [ "$(( ${EPOCHREALTIME/./} - started ))" -ge 1000000 ] || die "wait expired before one second passed"
  codex_run_alive "$run" || die "run stopped before wait expired"
  cat > "$work/watch.expected" <<'EXPECTED'
[STARTED] runner message
[DONE] turn complete
[NOTICE] unknown runner message
[WAITING] codex is still running; run the watch again
EXPECTED
  cmp -s "$work/watch.expected" "$work/watch.out" || die "unexpected bounded wait output: $(cat "$work/watch.out")"
  [ "$(cat "$run/progress.seen")" -eq 11 ] || die "wait counted its own [WAITING] message"
  pass

  case_name='wait returns at other decision lines'
  for line in '[STUCK?] repeated failure' '[QUIET] no events' '[FAILED] chosen failure'; do
    printf '%s\n' "$line" >> "$run/progress.log"
    timeout 5 "$watch" --wait 10 "$run" > "$work/watch.out" || die "wait did not return at $line"
    [ "$(cat "$work/watch.out")" = "$line" ] || die "wait lost or repeated a decision"
    codex_run_alive "$run" || die "run stopped before the decision"
  done
  "$stop" "$run" > "$work/stop.out" || die "could not stop wait test run"
  timeout 5 "$watch" --wait 10 "$run" > "$work/watch.out" || die "wait did not return at [EXIT]"
  [ "$(cat "$work/watch.out")" = '[EXIT] codex stopped by codex-stop.sh' ] || die "wait missed fresh [EXIT]"
  [ "$(cat "$run/progress.seen")" -eq "$(wc -l < "$run/progress.log")" ] || die "wait skipped decisions"
  pass

  new_run 'launcher returns during run'
  export FAKE_CODEX_SLEEP=30
  launch "$run" - </dev/null
  poll test -s "$run/thread"
  codex_run_alive "$run" || die "run already stopped"
  if finished "$run"; then die "launcher waited for completion"; fi
  sleeping="$run"
  pass

  case_name='watch re-arm preserves progress.seen'
  rc=0
  timeout 1 "$watch" "$sleeping" > "$work/watch.out" || rc=$?
  [ "$rc" -eq 124 ] || die "watch did not stay alive until timeout"
  [ "$(cat "$sleeping/progress.seen")" -eq 1 ] || die "watch did not save its position"
  pass

  new_run 'resume while alive refused'
  refuse "previous run $sleeping is still running; resumes to one codex session are serial" \
    --resume-from "$sleeping" "$run" resume 'follow up' </dev/null
  [ ! -e "$run/pid" ] || die "refused run started"
  pass

  new_run 'resume with --last refused'
  refuse '--resume-from cannot be used with --last' --resume-from "$previous" "$run" resume --last 'follow up' </dev/null
  [ ! -e "$run/pid" ] || die "refused run started"
  pass

  new_run 'resume without thread refused'
  refuse "no codex thread recorded in $work/missing" --resume-from "$work/missing" "$run" resume 'follow up' </dev/null
  [ ! -e "$run/pid" ] || die "refused run started"
  pass

  new_run 'resume with empty thread refused'
  mkdir -p "$work/empty"
  : > "$work/empty/thread"
  refuse "no codex thread recorded in $work/empty" --resume-from "$work/empty" "$run" resume 'follow up' </dev/null
  [ ! -e "$run/pid" ] || die "refused run started"
  pass

  new_run 'resume subcommand required'
  refuse '--resume-from requires a resume argument' --resume-from "$previous" "$run" 'please resume this' </dev/null
  [ ! -e "$run/pid" ] || die "refused run started"
  pass

  new_run 'resume passes recorded thread'
  launch --resume-from "$previous" "$run" --sandbox workspace-write resume 'follow up with spaces' </dev/null
  poll finished "$run"
  [ "$(cat "$run/resume.received")" = "$(cat "$previous/thread")" ] || die "wrong resume id"
  [ "$(cat "$run/prompt.received")" = 'follow up with spaces' ] || die "prompt changed"
  pass

  case_name='stop sleeping run and watch'
  timeout 12 "$watch" "$sleeping" > "$work/watch.out" &
  watch_pid=$!
  poll test -s "$sleeping/progress.seen"
  "$stop" "$sleeping" > "$work/stop.out" || die "stop failed"
  poll not_alive "$sleeping"
  poll group_gone "$sleeping"
  wait "$watch_pid" || die "watch did not finish normally after stop"
  watch_pid=""
  grep -q '^\[EXIT\] codex stopped by codex-stop.sh$' "$work/watch.out" || die "missing stop marker"
  [ "$(wc -l < "$work/watch.out")" -eq 1 ] || die "re-armed watch repeated progress"
  grep -q '^stopped ' "$work/stop.out" || die "stop did not identify the run"
  pass

  case_name='stop finished run'
  before=$(cat "$sleeping/progress.log")
  "$stop" "$sleeping" > "$work/stop.out" || die "second stop failed"
  grep -q '^run was not running:' "$work/stop.out" || die "inactive run reported as stopped"
  [ "$(cat "$sleeping/progress.log")" = "$before" ] || die "stop duplicated [EXIT]"
  pass

  for watch_mode in follow wait; do
    new_run "watch detects SIGKILL ($watch_mode)"
    export FAKE_CODEX_SLEEP=30
    launch "$run" </dev/null
    poll test -s "$run/thread"
    watch_options=()
    if [ "$watch_mode" = wait ]; then watch_options=(--wait 10); fi
    timeout 8 "$watch" "${watch_options[@]}" "$run" > "$work/watch.out" &
    watch_pid=$!
    poll test -s "$run/progress.seen"
    if [ "$mode" = systemd ]; then
      systemctl --user kill --signal=SIGKILL "$(cat "$run/unit")" || die "scope kill failed"
    else
      kill -KILL "$(cat "$run/pid")" || die "runner kill failed"
    fi
    rc=0
    wait "$watch_pid" || rc=$?
    watch_pid=""
    [ "$rc" -eq 1 ] || die "dead watch exited $rc instead of 1"
    grep -Fq "[FAILED] runner is gone without [EXIT]; see $run/runner.out and $run/stderr.log" "$work/watch.out" || die "missing death diagnostic"
    "$stop" "$run" > "$work/stop.out" || die "could not stop remaining children"
    poll group_gone "$run"
    pass
  done

  new_run 'chosen codex exit code'
  export FAKE_CODEX_RC=7
  launch "$run" </dev/null
  timeout 5 "$watch" "$run" > "$work/watch.out" || die "watch must exit 0 after any [EXIT]"
  grep -q '^\[EXIT\] codex rc=7$' "$work/watch.out" || die "lost codex exit code"
  pass
done
