{ inputs, ... }:

with inputs;

let hostname = "krupinski";
in {
  imports = [
    ./hardware-configuration.nix

    nixos-hardware.nixosModules.common-cpu-intel
    nixos-hardware.nixosModules.common-gpu-nvidia

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
    ../../profiles/dev.nix
    ../../profiles/work.nix

    ../../modules/displaylink.nix
  ];

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home;

  networking.hostName = hostname;

  hardware.nvidia.prime = {
    intelBusId = "PCI:0:2:0";
    nvidiaBusId = "PCI:1:0:0";
  };
}

