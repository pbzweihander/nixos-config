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

    # node
    nodejs
    nodePackages.typescript-language-server
    yarn

    # rust
    cargo
    cargo-edit
    cargo-sort
    rust-analyzer

    # aws
    awscli2

    # kubernetes
    kubectl
    kubectl-node-shell
    kubectx
    kubernetes-helm
  ];
}
