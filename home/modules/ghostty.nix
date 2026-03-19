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
      window-decoration = "none";

      keybind = [
        "ctrl+shift+h=goto_split:left"
        "ctrl+shift+j=goto_split:down"
        "ctrl+shift+k=goto_split:up"
        "ctrl+shift+l=goto_split:right"
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
