{ pkgs, ... }:

{
  imports = [
    ../modules/rust.nix
    ../modules/podman.nix
    ../modules/awscli.nix
    ../modules/kubectl.nix
  ];

  environment.systemPackages = with pkgs; [ devenv ];
}
