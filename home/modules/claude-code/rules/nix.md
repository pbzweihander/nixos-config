---
paths:
  - "**/*.nix"
---

# Nix code

- After changing Nix code, lint it with statix and fix every warning before finishing:
  `statix check <path>` (or `nix run nixpkgs#statix -- check <path>` where statix is
  not installed). `statix fix <path>` applies the automatic fixes; review its diff.
- When the repository's CI runs statix with extra flags, use the same flags. In
  nixos-config: `statix check --ignore hardware-configuration.nix`.
- Also run the project's formatter, such as `nix fmt` in a flake that defines one.
