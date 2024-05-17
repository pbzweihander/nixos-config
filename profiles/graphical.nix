{
  imports = [ ../modules/kde.nix ../modules/sound.nix ../modules/fonts.nix ];

  services.xserver = {
    enable = true;
    xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
  };
}
