{ pkgs, ... }:

{
  home.packages = with pkgs; [
    pulseaudioFull
  ];

  programs = {
    firefox.enable = true;
    alacritty.enable = true;
  };

  programs.plasma = {
    enable = true;

    workspace = {
      clickItemTo = "select";
      wallpaperPictureOfTheDay.provider = "apod";
    };

    panels = [
      {
        location = "right";
        widgets = [
          {
            name = "org.kde.plasma.kickoff";
          }
          {
            name = "org.kde.plasma.icontasks";
            config = {
              General.launchers = [
                "applications:firefox"
                "applications:org.kde.dolphin.desktop"
              ];
            };
          }
          "org.kde.plasma.marginsseparator"
          "org.kde.plasma.systemtray"
          {
            digitalClock = {
              format = "MM/dd";
              time.format = "24h";
            };
          }
        ];
      }
    ];

    shortcuts = {
      ksmserver = {
        "Lock Session" = [ "Screensaver" "Meta+Esc" ];
      };
      "services/Alacritty.desktop"."New" = "Meta+Return";
    };

    configFile = {
      "kwinrc"."Effect-overview"."BorderActivate" = 9;
    };
  };
}

