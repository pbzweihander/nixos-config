{ pkgs, ... }:

{
  imports = [
    ../../home/profiles/basic.nix
    ../../home/profiles/graphical.nix
    ../../home/profiles/kde.nix
    ../../home/profiles/gaming.nix
  ];

  xdg.desktopEntries = {
    # Use steam launch option `DISPLAY=:1 %command%`
    gamescope = {
      name = "Gamescope";
      exec =
        "${pkgs.coreutils}/bin/env DXVK_HDR=1 ${pkgs.gamescope}/bin/gamescope --borderless --fullscreen -W 3840 -H 2160 -r 160 --hdr-enabled --adaptive-sync --prefer-output DP-1";
      terminal = false;
      categories = [ "Game" ];
    };
  };
}

