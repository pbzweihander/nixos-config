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
    ../../profiles/kde.nix
    ../../profiles/dev.nix
    ../../profiles/gaming.nix
  ];

  services = {
    btrfs.autoScrub.enable = true;
    cloudflare-warp.enable = true;
  };

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  networking = {
    hostName = hostname;
    firewall.allowedUDPPorts = [ 2934 2935 9987 9988 9989 ];
  };

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

  environment.systemPackages = with pkgs; [ opentrack ];
}
