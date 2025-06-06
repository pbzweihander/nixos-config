{ pkgs, ... }:
{
  imports = [
    ../modules/container.nix
    ../modules/pueue.nix
  ];

  programs.nix-ld.enable = true;

  environment.systemPackages = with pkgs; [
    # go
    go
    golangci-lint
    golangci-lint-langserver
    gopls

    # node
    nodejs
    nodePackages.typescript-language-server
    pnpm
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
    kubectx
    kubernetes-helm
    yaml-language-server

    # etc
    yamlfmt
  ];
}
