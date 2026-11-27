{ pkgs, ... }:
{
  # Not installed for now: the rossmann SSH launcher is no longer needed.
  # The source stays in ./ghostty-remote.nix; uncomment to bring it back.
  # imports = [ ./ghostty-remote.nix ];

  programs.ghostty = {
    enable = true;
    settings = {
      app-notifications = "no-clipboard-copy";
      # A bell (Claude Code's notifications ring one) also plays a sound; attention
      # and title are the defaults: taskbar highlight and a 🔔 on the tab.
      bell-features = "audio,attention,title";
      bell-audio-path = "${pkgs.sound-theme-freedesktop}/share/sounds/freedesktop/stereo/complete.oga";
      bell-audio-volume = 0.5;
      background-opacity = 0.9;
      copy-on-select = false;
      font-family = "Sarasa Term K";
      gtk-titlebar-style = "tabs";
      resize-overlay = "never";
      shell-integration-features = "cursor,no-sudo,title,ssh-env,ssh-terminfo,path";
      window-decoration = "client";

      keybind = [
        "ctrl+shift+enter=new_split:right"
        "ctrl+shift+alt+enter=new_split:down"
        "ctrl+shift+h=goto_split:left"
        "ctrl+shift+j=goto_split:down"
        "ctrl+shift+k=goto_split:up"
        "ctrl+shift+l=goto_split:right"
        "ctrl+shift+q=unbind"
        "ctrl+enter=unbind"
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
}
