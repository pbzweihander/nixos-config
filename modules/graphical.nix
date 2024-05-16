{
  imports = [ ./kde.nix ];

  services.xserver = {
    enable = true;
    xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
  };
}
