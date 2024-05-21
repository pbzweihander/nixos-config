let hostname = "krupinski";
in {
  imports = [
    ./hardware-configuration.nix

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
    ../../profiles/dev.nix
    ../../profiles/work.nix
  ];

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home;

  networking.hostName = hostname;
}

