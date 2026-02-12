{
  inputs,
  pkgs,
  ...
}:
with inputs;
let
  hostname = "schnaufer";
in
{
  imports = [
    ./hardware-configuration.nix

    nixos-apple-silicon.nixosModules.default

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
    ../../profiles/dev.nix
    ../../profiles/work.nix

    ../../modules/kde.nix
    ../../modules/photo.nix
    ../../modules/wifi.nix
  ];

  nixpkgs.hostPlatform = "aarch64-linux";

  services = {
    btrfs.autoScrub = {
      enable = true;
      fileSystems = [ "/" ];
    };
    fstrim.enable = true;
    xserver.xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
  };

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  networking.hostName = hostname;

  nix.settings = {
    extra-substituters = [ "https://nixos-apple-silicon.cachix.org" ];
    extra-trusted-public-keys = [
      "nixos-apple-silicon.cachix.org-1:8psDu5SA5dAD7qA0zMy5UT292TxeEPzIz8VVEr2Js20="
    ];
  };

  hardware = {
    bluetooth.enable = true;
    asahi = {
      enable = true;
      peripheralFirmwareDirectory = pkgs.requireFile {
        name = "asahi";
        hashMode = "recursive";
        hash = "sha256:0rqqg1lafmk4fgjqbm0n2zyiv4p4d337h596p83bd3vwslkrri17";
        message = ''
          nix-store --add-fixed sha256 --recursive <efi>/asahi
          nix-store --query --hash /nix/<output>
        '';
      };
    };
  };
}
