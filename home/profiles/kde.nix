{
  # TODO: Replace with home-manager fcitx5 settings when 25.11
  xdg.configFile = {
    "fcitx5/config".text = ''
      [Hotkey/TriggerKeys]
      0=Hangul

      [Hotkey/ActivateKeys]
      0=Shift+space

      [Hotkey/DeactivateKeys]
      0=Control+space
      1=Escape

      [Behavior]
      ShowInputMethodInformation=False
    '';
    "fcitx5/profile".text = ''
      [Groups/0]
      Name=Default
      Default Layout=us
      DefaultIM=hangul

      [Groups/0/Items/0]
      Name=keyboard-us
      Layout=

      [Groups/0/Items/1]
      Name=hangul
      Layout=

      [GroupOrder]
      0=Default
    '';
  };

  # i18n.inputMethod = {
  #   enable = true;
  #   type = "fcitx5";
  #   fcitx5 = {
  #     addons = with pkgs; [fcitx5-gtk fcitx5-hangul fcitx5-configtool];
  #     settings = {
  #       globalOptions = {
  #         "HotKey/TriggerKeys"."0" = "Hangul";
  #         "HotKey/ActivateKeys"."0" = "Shift+Hangul";
  #         "HotKey/DeactivateKeys" = {
  #           "0" = "Control+Hangul";
  #           "1" = "Escape";
  #         };
  #         Behavior.ShowInputMethodInformation = false;
  #       };
  #       inputMethod = {
  #         GroupOrder."0" = "Default";
  #         "Groups/0" = {
  #           Name = "Default";
  #           "Default Layout" = "us";
  #           DefaultIM = "hangul";
  #         };
  #         "Groups/0/Items/0".Name = "keyboard-us";
  #         "Groups/0/Items/1".Name = "hangul";
  #       };
  #     };
  #     waylandFrontend = true;
  #   };
  # };

  # plasma-manager
  programs.plasma = {
    enable = true;
    workspace = {
      clickItemTo = "select";
      wallpaperPictureOfTheDay.provider = "apod";
    };
    shortcuts = {
      ksmserver = {"Lock Session" = ["Screensaver" "Meta+Esc"];};
      "services/Alacritty.desktop"."New" = "Meta+Return";
    };
    configFile = {
      "dolphinrc"."DetailsMode"."ExpandableFolders" = false;
      "kdeglobals"."General"."TerminalApplication" = "alacritty";
      "kwinrc"."Effect-overview"."BorderActivate" = 9;
      "kwinrc"."Wayland"."InputMethod[$e]" = "/run/current-system/sw/share/applications/fcitx5-wayland-launcher.desktop";
      "ksmserverrc"."General"."confirmLogout" = false;
    };
  };
}
