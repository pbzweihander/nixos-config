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
    podman = {
      enable = true;
      defaultNetwork.settings.dns_enabled = true;
    };
    oci-containers.backend = "podman";
  };
  users.users.pbzweihander = {
    subUidRanges = [
      {
        startUid = 100000;
        count = 65536;
      }
    ];
    subGidRanges = [
      {
        startGid = 100000;
        count = 65536;
      }
    ];
  };

  environment.systemPackages = with pkgs; [
    kind
    podman-compose
  ];
}
