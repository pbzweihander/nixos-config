#!/usr/bin/env bash
# Run the latest codex from nixpkgs nixos-unstable; fall back to the locally
# installed codex only when the nix build itself fails.
set -euo pipefail

flake_ref="github:nixos/nixpkgs/nixos-unstable#codex"

log() { printf '[codex-latest] %s\n' "$*" >&2; }

if [ "${CODEX_LATEST_FORCE_LOCAL:-}" = "1" ]; then
  log "CODEX_LATEST_FORCE_LOCAL=1, using local codex"
  exec codex "$@"
fi

log "building $flake_ref ..."
if out="$(nix build --no-link --print-out-paths "$flake_ref")" && [ -x "$out/bin/codex" ]; then
  log "using $out/bin/codex ($("$out/bin/codex" --version 2>/dev/null || echo unknown))"
  exec "$out/bin/codex" "$@"
fi

if command -v codex >/dev/null 2>&1; then
  log "nix build failed, falling back to local codex ($(codex --version 2>/dev/null || echo unknown))"
  exec codex "$@"
fi

log "nix build failed and no local codex found"
exit 127
