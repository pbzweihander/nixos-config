{
  pkgs,
  inputs,
  ...
}:
with inputs;
let
  hostname = "linnamaa";
in
{
  imports = [
    ./hardware-configuration.nix
    nixos-hardware.nixosModules.lenovo-thinkpad-x13-yoga

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
    ../../profiles/dev.nix

    ../../modules/kde.nix
    ../../modules/wifi.nix
  ];

  nixpkgs.hostPlatform = "x86_64-linux";

  services = {
    btrfs.autoScrub = {
      enable = true;
      fileSystems = [ "/" ];
    };
    xserver.xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
  };

  environment.systemPackages = with pkgs; [ libmbim ];

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  systemd.services = {
    modem = {
      enable = true;
      path = [ pkgs.modemmanager ];
      wantedBy = [ "multi-user.target" ];
    };
    reload-modem = {
      enable = true;
      after = [
        "suspend.target"
        "hibernate.target"
      ];
      script = "systemctl restart ModemManager.service";
      wantedBy = [
        "suspend.target"
        "hibernate.target"
      ];
    };
  };

  networking = {
    hostName = hostname;
    networkmanager = {
      ensureProfiles.profiles.wwan = {
        connection = {
          id = "WWAN";
          type = "gsm";
        };
        gsm = {
          apn = "lte.ktfwing.com";
          number = "*99#";
          password-flags = "4";
          pin-flags = "4";
        };
        ipv4 = {
          method = "auto";
        };
        ipv6 = {
          addr-gen-mode = "stable-privacy";
          method = "auto";
        };
        ppp = { };
        proxy = { };
      };
    };
    modemmanager.fccUnlockScripts = [
      {
        id = "2c7c:030a";
        path = ./fcc-unlock.sh;
      }
    ];
  };
}
