{ pkgs, ... }:

{
  fonts = {
    fontDir.enable = true;
    packages = with pkgs; [
      iosevka
      noto-fonts-cjk-sans
      sarasa-gothic
      source-han-sans
      (nerdfonts.override { fonts = [ "Iosevka" ]; })
    ];
    fontconfig = {
      enable = true;
      defaultFonts = {
        serif = [ "Sarasa Gothic K" ];
        sansSerif = [ "Sarasa UI K" ];
        monospace = [ "Sarasa Mono K" ];
      };
    };
  };
}

