{ pkgs, inputs, ... }:

with inputs;

let hostname = "rossmann";
in {
  imports = [
    ./hardware-configuration.nix
    nixos-hardware.nixosModules.lenovo-thinkpad-x13-yoga

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
  ];

  networking = { hostName = hostname; };

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home;

  systemd.services.modem = {
    path = [ pkgs.modemmanager ];
    wantedBy = [ "multi-user.target" ];
  };
}

