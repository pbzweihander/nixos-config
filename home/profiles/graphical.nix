{ pkgs, ... }:

{
  imports = [ ../modules/alacritty ../modules/fcitx5 ];

  home.packages = with pkgs; [ pulseaudioFull libnotify ];

  programs.firefox.enable = true;

  programs.plasma = {
    enable = true;
    workspace = {
      clickItemTo = "select";
      wallpaperPictureOfTheDay.provider = "apod";
    };
    shortcuts = {
      ksmserver = { "Lock Session" = [ "Screensaver" "Meta+Esc" ]; };
      "services/Alacritty.desktop"."New" = "Meta+Return";
    };
    configFile = {
      "dolphinrc"."DetailsMode"."ExpandableFolders" = false;
      "kdeglobals"."General"."TerminalApplication" = "alacritty";
      "kwinrc"."Effect-overview"."BorderActivate" = 9;
      "kwinrc"."Wayland"."InputMethod[$e]" =
        "/etc/profiles/per-user/pbzweihander/share/applications/org.fcitx.Fcitx5.desktop";
      "kwinrc"."Wayland"."InputMethodx5b$ex5d" =
        "/etc/profiles/per-user/pbzweihander/share/applications/org.fcitx.Fcitx5.desktop";
      "ksmserverrc"."General"."confirmLogout" = false;
    };
  };
}

