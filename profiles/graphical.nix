{ pkgs, ... }:
{
  imports = [
    ../modules/sound.nix
    ../modules/fonts.nix
  ];

  environment.systemPackages = with pkgs; [
    alacritty
    gimp
    inkscape
    kcalc
    spotify
    unstable.zed-editor
    vlc
    vscodium
    wl-clipboard
  ];

  services = {
    xserver = {
      enable = false;
      xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
      excludePackages = with pkgs; [ xterm ];
    };
  };

  programs.firefox.enable = true;
}
