{
  inputs,
  ...
}:
with inputs;
let
  hostname = "juutilainen";
in
{
  imports = [
    ./hardware-configuration.nix

    nixos-hardware.nixosModules.lenovo-thinkpad-x1-12th-gen

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
    ../../profiles/dev.nix
    ../../profiles/work.nix

    ../../modules/kde.nix
    ../../modules/via.nix
  ];

  services = {
    btrfs.autoScrub.enable = true;
    fstrim.enable = true;
    xserver.xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
    tailscale.enable = true;
  };

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  networking.hostName = hostname;

  hardware.bluetooth.enable = true;

  boot.kernel.sysctl = {
    "vm.max_map_count" = 16777216;
    "fs.file-max" = 524288;
  };
}
