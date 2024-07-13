{ pkgs, ... }:

{
  imports = [ ../modules/kde.nix ../modules/sound.nix ../modules/fonts.nix ];

  environment.systemPackages = with pkgs; [
    alacritty
    bitwarden
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
      enable = true;
      xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
    };
    udev.packages = with pkgs; [ via vial ];
  };
}
