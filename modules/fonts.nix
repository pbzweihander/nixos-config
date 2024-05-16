{ pkgs, ... }:

{
  fonts = {
    fontDir.enable = true;
    packages = with pkgs; [ sarasa-gothic source-han-sans iosevka ];
  };
}

