{ pkgs, ... }:
{
  imports = [
    ../modules/container.nix
    ../modules/pueue.nix
  ];

  programs.nix-ld.enable = true;

  # The GNOME Orca screen reader (enabled by default with Plasma) also installs
  # `orca` and an "Orca" desktop entry, which clash with the Orca IDE's.
  services.orca.enable = false;

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
      kubelogin-oidc
      kubernetes-helm
      yaml-language-server

      # shell
      bash-language-server
      shfmt

      # agent
      claude-code
      codex
      gemini-cli

      # etc
      gh
      orca-ide
      yamlfmt
      (mdformat.withPlugins (
        ps: with ps; [
          mdformat-gfm
        ]
      ))
    ]

  services.tailscale.enable = true;
}
