{ pkgs, ... }:

{
  imports =
    [ ../../../home/profiles/basic.nix ../../../home/profiles/graphical.nix ];

  xdg.desktopEntries = {
    steam-gamescope = {
      name = "Steam (Gamescope)";
      exec =
        "gamescope --borderless --fullscreen -W 3840 -H 2160 -r 160 --hdr-enabled --steam --backend wayland -- steam -pipewire-dmabuf %U";
      terminal = false;
      icon = "steam";
      categories = [ "Network" "FileTransfer" "Game" ];
    };
  };

  systemd.user.services = {
    gamescope = {
      Service = {
        Type = "simple";
        Restart = "always";
        ExecStart =
          "${pkgs.gamescope}/bin/gamescope --borderless --fullscreen -W 3840 -H 2160 -r 160 --hdr-enabled --steam --backend wayland";
      };
    };
  };
}

