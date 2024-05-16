{
  programs.alacritty.enable = true;

  home.file = {
    ".config/alacritty/alacritty.toml".source = ./alacritty.toml;
    ".config/alacritty/dimidium.toml".source = ./dimidium.toml;
  };
}
