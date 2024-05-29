{ pkgs, ... }:

{
  imports = [ ../modules/kde.nix ../modules/sound.nix ../modules/fonts.nix ];

  environment.systemPackages = with pkgs; [
    alacritty
    bitwarden
    logseq
    spotify
    wl-clipboard
  ];

  services.xserver = {
    enable = true;
    xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
  };
}
