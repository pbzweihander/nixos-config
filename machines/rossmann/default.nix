{ inputs, ... }:

with inputs;

let hostname = "rossmann";
in {
  imports = [
    ./hardware-configuration.nix

    nixos-hardware.nixosModules.common-cpu-amd
    nixos-hardware.nixosModules.common-gpu-amd

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
    ../../profiles/dev.nix
  ];

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home;

  networking.hostName = hostname;
}

