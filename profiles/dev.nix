{ pkgs, ... }:

{
  imports = [
    ../modules/rust.nix
    ../modules/container.nix
    ../modules/awscli.nix
    ../modules/kubectl.nix
  ];

  environment.systemPackages = with pkgs; [ devenv nodejs_22 yarn ];
}
