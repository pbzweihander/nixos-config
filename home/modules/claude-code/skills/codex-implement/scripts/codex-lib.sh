#!/usr/bin/env bash
# Shared liveness check: a scope owns all descendants, otherwise the runner leads
# a detached process group. A recorded scope always takes precedence over the pid.
set -uo pipefail

codex_run_alive() {
  local dir="$1" unit pid
  if [ -f "$dir/unit" ]; then
    unit=$(cat "$dir/unit") || return 1
    [ -n "$unit" ] && systemctl --user is-active --quiet "$unit" >/dev/null 2>&1
  else
    pid=$(cat "$dir/pid" 2>/dev/null) || return 1
    [[ "$pid" =~ ^[0-9]+$ ]] && [ "$pid" -gt 1 ] && kill -0 "$pid" 2>/dev/null
  fi
}
