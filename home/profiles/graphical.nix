{ pkgs, ... }:

{
  imports = [ ../modules/alacritty ];

  home.packages = with pkgs; [ pulseaudioFull libnotify ];

  programs.firefox.enable = true;
}

