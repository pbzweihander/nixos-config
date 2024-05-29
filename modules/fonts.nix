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
  };
}

