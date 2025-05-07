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
    vlc
    wl-clipboard
    unstable.zed-editor
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
