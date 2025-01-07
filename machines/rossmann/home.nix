{ pkgs, ... }:

{
  imports = [
    ../../home/profiles/basic.nix
    ../../home/profiles/graphical.nix
    ../../home/profiles/kde.nix
    ../../home/profiles/gaming.nix
  ];

  xdg.desktopEntries = {
    steam-gamescope = {
      name = "Steam (Gamescope)";
      exec =
        "gamescope --borderless --fullscreen -W 3840 -H 2160 -r 160 --hdr-enabled --steam --backend wayland -- steam -pipewire-dmabuf %U";
      terminal = false;
      icon = "steam";
      categories = [ "Game" ];
    };
  };

  systemd.user.services = {
    gamescope = {
      Unit = { After = [ "graphical.target" ]; };
      Service = {
        Type = "simple";
        Restart = "always";
        ExecStart =
          "${pkgs.gamescope}/bin/gamescope --borderless --fullscreen -W 3840 -H 2160 -r 160 --hdr-enabled --steam --backend wayland";
      };
      Install = { WantedBy = [ "default.target" ]; };
    };
  };
}

