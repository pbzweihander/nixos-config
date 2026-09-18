{ config, ... }:
{
  programs.ghostty = {
    enable = true;
    settings = {
      app-notifications = "no-clipboard-copy";
      background-opacity = 0.9;
      copy-on-select = false;
      font-family = "Sarasa Term K";
      gtk-titlebar-style = "tabs";
      resize-overlay = "never";
      shell-integration-features = "cursor,no-sudo,title,ssh-env,ssh-terminfo,path";
      window-decoration = "client";

      keybind = [
        "ctrl+alt+enter=new_split:right"
        "ctrl+shift+alt+enter=new_split:down"
        "ctrl+shift+h=goto_split:left"
        "ctrl+shift+j=goto_split:down"
        "ctrl+shift+k=goto_split:up"
        "ctrl+shift+l=goto_split:right"
        "ctrl+shift+q=unbind"
      ];

      foreground = "#bab7b6";
      background = "#141414";
      cursor-color = "#37e57b";
      cursor-text = "#141414";
      selection-background = "#8db8e5";
      selection-foreground = "#141414";
      palette = [
        "0=#000000"
        "1=#cf494c"
        "2=#60b442"
        "3=#db9c11"
        "4=#0575d8"
        "5=#af5ed2"
        "6=#1db6bb"
        "7=#bab7b6"
        "8=#817e7e"
        "9=#ff643b"
        "10=#37e57b"
        "11=#fccd1a"
        "12=#688dfd"
        "13=#ed6fe9"
        "14=#32e0fb"
        "15=#dee3e4"
      ];
    };
  };

  # `ghostty --config-file=.../ghostty/remote` starts a window whose *every* new
  # tab and split runs this command, so a remote session does not have to be
  # started by hand in each one. Loaded on top of the normal config.
  xdg.configFile."ghostty/remote".text = ''
    command = ssh rossmann
  '';

  xdg.desktopEntries.ghostty-rossmann = {
    name = "Ghostty (rossmann)";
    genericName = "Terminal";
    comment = "Ghostty window where every tab and split opens an SSH session on rossmann";
    exec = "ghostty --config-file=${config.xdg.configHome}/ghostty/remote --gtk-single-instance=false";
    icon = "com.mitchellh.ghostty";
    terminal = false;
    categories = [
      "System"
      "TerminalEmulator"
    ];
  };
}
