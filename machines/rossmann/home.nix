{ pkgs, inputs, ... }:
{
  imports = [
    ../../home/profiles/basic.nix
    ../../home/profiles/graphical.nix
    ../../home/profiles/kde.nix
    ../../home/profiles/gaming.nix
  ];

  systemd.user.services = {
    # Use steam launch option `DISPLAY=:1 %command%`
    gamescope = {
      Service = {
        Type = "simple";
        Restart = "always";
        ExecStart = "${pkgs.gamescope}/bin/gamescope --borderless --fullscreen -W 3840 -H 2160 -r 160 --hdr-enabled --backend wayland --adaptive-sync --prefer-output DP-1";
        Environment = "DXVK_HDR=1";
      };
    };
  };

  home.file.".links/nix-citizen-wine-astral/bin".source = "${
    inputs.nix-citizen.packages.${pkgs.system}.wine-astral
  }/bin";
}
