#!/usr/bin/env bash
# Stop every process owned by a run, then let any Monitor watch finish.
# usage: codex-stop.sh <run-dir>
set -uo pipefail
here="$(dirname "$(readlink -f "$0")")"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=codex-lib.sh
source "$here/codex-lib.sh"
dir="$1"

# A killed runner may leave children in its process group. Stop those too, even
# though the shared liveness check correctly reports that the runner is gone.
pid=""
if [ ! -f "$dir/unit" ]; then pid=$(cat "$dir/pid" 2>/dev/null) || pid=""; fi
if codex_run_alive "$dir" || { [[ "$pid" =~ ^[0-9]+$ ]] && [ "$pid" -gt 1 ] && kill -0 -- "-$pid" 2>/dev/null; }; then
  if [ -f "$dir/unit" ]; then
    unit=$(cat "$dir/unit")
    systemctl --user stop "$unit" || exit 1
    stopped="$unit"
  else
    kill -TERM -- "-$pid" 2>/dev/null
    # Check the group too: a child may ignore TERM after the runner has exited.
    deadline=$((SECONDS + 5))
    while kill -0 -- "-$pid" 2>/dev/null && [ "$SECONDS" -lt "$deadline" ]; do sleep 0.1; done
    if kill -0 -- "-$pid" 2>/dev/null; then kill -KILL -- "-$pid" 2>/dev/null; fi
    stopped="process group $pid"
  fi
  message="stopped $stopped"
else
  message="run was not running: $dir"
fi

if ! grep -q '^\[EXIT\]' "$dir/progress.log"; then
  printf '[EXIT] codex stopped by codex-stop.sh\n' >> "$dir/progress.log"
fi
printf '%s\n' "$message"
exit 0
