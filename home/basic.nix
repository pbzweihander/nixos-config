{
  imports = [
    ./fish
    ./git
    ./helix
  ];

  home = {
    stateVersion = "23.11";

    username = "pbzweihander";
    homeDirectory = "/home/pbzweihander";
  };

  fonts.fontconfig.enable = true;

  xdg.enable = true;

  programs = {
    home-manager.enable = true;
    git.enable = true;
    vim.enable = true;
    helix.enable = true;
  };
}
