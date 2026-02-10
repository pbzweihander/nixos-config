{ pkgs, ... }:
{
  imports = [
    ../modules/sound.nix
    ../modules/fonts.nix
  ];

  environment = {
    systemPackages = with pkgs; [
      alacritty
      gimp
      inkscape
      kdePackages.kcalc
      spotify-qt
      vlc
      vscode
      wl-clipboard
    ];
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
