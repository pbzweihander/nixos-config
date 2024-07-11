{ pkgs, ... }:

{
  fonts = {
    fontDir.enable = true;
    packages = with pkgs; [
      iosevka
      nerdfonts
      noto-fonts-cjk-sans
      sarasa-gothic
      source-han-sans
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

