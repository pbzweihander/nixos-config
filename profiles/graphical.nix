{ pkgs, ... }:
{
  imports = [
    ../modules/sound.nix
    ../modules/fonts.nix
  ];

  environment = {
    systemPackages =
      with pkgs;
      [
        alacritty
        ghostty
        gimp
        inkscape
        kdePackages.kcalc
        vlc
        vscode
        wl-clipboard
      ]
      ++ (
        if stdenv.hostPlatform.system == "aarch64-linux" then
          [
            librespot
            spotify-qt
          ]
        else
          [ spotify ]
      );
    sessionVariables.NIXOS_OZONE_WL = "1";
  };

  services = {
    xserver = {
      enable = false;
      excludePackages = with pkgs; [ xterm ];
    };
  };

  programs.firefox.enable = true;
}
