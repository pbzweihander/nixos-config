{ pkgs, inputs, ... }:

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
    ../../profiles/gaming.nix

    nix-citizen.nixosModules.StarCitizen
  ];

  services.btrfs.autoScrub.enable = true;

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  networking.hostName = hostname;

  programs.steam.gamescopeSession = {
    env = {
      ENABLE_HDR_WSI = "1";
      ENABLE_GAMESCOPE_WSI = "1";
      DXVK_HDR = "1";
    };
    args = [
      "--expose-wayland"
      "--hdr-enabled"
      "--hdr-debug-force-output"
      "--prefer-output=DP-1"
    ];
  };

  nix.settings = {
    substituters =
      [ "https://nix-gaming.cachix.org" "https://nix-citizen.cachix.org" ];
    trusted-public-keys = [
      "nix-gaming.cachix.org-1:nbjlureqMbRAxR1gJ/f3hxemL9svXaZF/Ees8vCUUs4="
      "nix-citizen.cachix.org-1:lPMkWc2X8XD4/7YPEEwXKKBg+SVbYTVrAaLA2wQTKCo="
    ];
  };
  nixpkgs.overlays = [ nix-citizen.overlays.default ];

  nix-citizen.starCitizen = {
    enable = true;
    gplAsync.enable = true;
    helperScript.enable = true;
  };

  environment.systemPackages = with pkgs; [ opentrack ];
}
