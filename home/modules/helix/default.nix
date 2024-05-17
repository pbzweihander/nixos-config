{
  programs.helix = {
    enable = true;
    defaultEditor = true;
  };

  home.file = {
    ".config/helix/config.toml".source = ./config.toml;
    ".config/helix/languages.toml".source = ./languages.toml;
    ".config/helix/themes/base16.toml".source = ./themes/base16.toml;
  };
}
