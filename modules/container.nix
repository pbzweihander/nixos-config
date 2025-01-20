{ pkgs, ... }:

{
  virtualisation = {
    containers = {
      enable = true;
      registries = {
        insecure = [ "localhost:5001" ];
        search = [ "docker.io" ];
      };
    };
    podman.enable = true;
  };

  environment.systemPackages = with pkgs; [ kind podman-compose ];
}
