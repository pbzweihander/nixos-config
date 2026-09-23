{ pkgs, config, ... }:
{
  imports = [
    ../modules/container.nix
    ../modules/pueue.nix
  ];

  programs.nix-ld.enable = true;

  environment.systemPackages =
    with pkgs;
    [
      # go
      go
      golangci-lint
      golangci-lint-langserver
      gopls

      # node
      nodejs
      pnpm
      typescript-language-server
      yarn

      # rust
      cargo
      cargo-edit
      cargo-sort
      rust-analyzer

      # aws
      awscli2

      # kubernetes
      helm-ls
      kubectl
      kubectl-node-shell
      kubectl-view-allocations
      kubectx
      kubernetes-helm
      yaml-language-server

      # shell
      bash-language-server
      shfmt

      # etc
      yamlfmt
      (mdformat.withPlugins (
        ps: with ps; [
          mdformat-gfm
        ]
      ))
    ]
    ++ (with config.multiverse.instance.tip; [
      codex
      # Fallback only. Claude Code is normally the self-updating native build in
      # ~/.local/share/claude/versions, installed with `claude install latest` and
      # reached through ~/.local/bin/claude, which fish puts ahead of the system
      # profile in PATH. The native ELF needs only glibc and runs unpatched thanks
      # to nix-ld. This package is what `claude` resolves to when that is absent.
      claude-code
    ]);

  services.tailscale.enable = true;
}
