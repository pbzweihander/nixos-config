{ pkgs, ... }:

{
  virtualisation = {
    containers.enable = true;
    podman.enable = true;
  };

  environment.systemPackages = with pkgs; [ kind ];
}
