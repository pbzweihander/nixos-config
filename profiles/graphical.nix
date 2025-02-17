{ pkgs, ... }:

{
  imports = [ ../modules/sound.nix ../modules/fonts.nix ];

  environment.systemPackages = with pkgs; [
    alacritty
    gimp
    inkscape
    keymapp
    # logseq TODO: outdated electron
    spotify
    uhk-agent
    uhk-udev-rules
    vial
    wl-clipboard
    zsa-udev-rules
  ];

  services = {
    xserver = {
      enable = false;
      xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
      excludePackages = with pkgs; [ xterm ];
    };
    udev.packages = with pkgs; [ via vial ];
  };

  programs.firefox.enable = true;
}
