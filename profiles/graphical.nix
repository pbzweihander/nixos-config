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
      kcalc
      spotify
      unstable.zed-editor
      vlc
      vscode
      wl-clipboard
    ];
    sessionVariables.NIXOS_OZONE_WL = "1";
  };

  services = {
    xserver = {
      enable = false;
      xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
      excludePackages = with pkgs; [ xterm ];
    };
  };

  programs.firefox.enable = true;
}
