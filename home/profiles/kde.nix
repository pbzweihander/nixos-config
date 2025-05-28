{ pkgs, ... }:
{
  i18n.inputMethod = {
    enable = true;
    type = "fcitx5";
    fcitx5 = {
      addons = with pkgs; [
        fcitx5-gtk
        fcitx5-hangul
        fcitx5-configtool
      ];
      # TODO: declarative settings when home-manager fixed
      # settings = {
      #   globalOptions = {
      #     "HotKey/TriggerKeys"."0" = "Hangul";
      #     "HotKey/ActivateKeys"."0" = "Shift+Hangul";
      #     "HotKey/DeactivateKeys" = {
      #       "0" = "Control+Hangul";
      #       "1" = "Escape";
      #     };
      #     Behavior.ShowInputMethodInformation = false;
      #   };
      #   inputMethod = {
      #     GroupOrder."0" = "Default";
      #     "Groups/0" = {
      #       Name = "Default";
      #       "Default Layout" = "us";
      #       DefaultIM = "hangul";
      #     };
      #     "Groups/0/Items/0".Name = "keyboard-us";
      #     "Groups/0/Items/1".Name = "hangul";
      #   };
      # };
      waylandFrontend = true;
    };
  };

  # plasma-manager
  programs.plasma = {
    enable = true;
    kscreenlocker.appearance.wallpaperPictureOfTheDay.provider = "apod";
    krunner.shortcuts.launch = [
      "Search"
      "Meta+Space"
      "Alt+F2"
    ];
    fonts = {
      general = {
        family = "Sarasa UI K";
        pointSize = 10;
      };
      fixedWidth = {
        family = "Sarasa Fixed K";
        pointSize = 10;
      };
      small = {
        family = "Sarasa UI K";
        pointSize = 8;
      };
      toolbar = {
        family = "Sarasa UI K";
        pointSize = 10;
      };
      menu = {
        family = "Sarasa UI K";
        pointSize = 10;
      };
      windowTitle = {
        family = "Sarasa UI K";
        pointSize = 10;
      };
    };
    session.general.askForConfirmationOnLogout = false;
    shortcuts = {
      ksmserver = {
        "Lock Session" = [
          "Screensaver"
          "Meta+Esc"
        ];
      };
      "services/Alacritty.desktop"."New" = "Meta+Return";
    };
    spectacle.shortcuts = {
      captureEntireDesktop = "Meta+Shift+Print";
      captureRectangularRegion = "Shift+Print";
      captureWindowUnderCursor = "Ctrl+Print";
    };
    workspace = {
      clickItemTo = "select";
      wallpaperPictureOfTheDay.provider = "apod";
    };
    configFile = {
      "dolphinrc"."DetailsMode"."ExpandableFolders" = false;
      "kdeglobals"."General"."TerminalApplication" = "alacritty";
      "kwinrc"."Effect-overview"."BorderActivate" = 9;
      "kwinrc"."Wayland"."InputMethod[$e]" =
        "/run/current-system/sw/share/applications/fcitx5-wayland-launcher.desktop";
    };
  };
}
