{ pkgs, ... }:

{
  imports = [
    ../modules/kde.nix
    ../modules/sound.nix
    ../modules/fonts.nix
    ../modules/logseq.nix
  ];

  environment.systemPackages = with pkgs; [ wl-clipboard alacritty ];

  services.xserver = {
    enable = true;
    xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
  };
}
