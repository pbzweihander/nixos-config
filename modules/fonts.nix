{ pkgs, ... }:
{
  fonts = {
    fontDir.enable = true;
    packages = with pkgs; [
      iosevka
      noto-fonts-cjk-sans
      sarasa-gothic
      source-han-sans
      nerd-fonts.iosevka
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
