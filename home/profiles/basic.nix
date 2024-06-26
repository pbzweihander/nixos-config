{
  imports = [ ../modules/fish ../modules/git ../modules/helix ../modules/ssh ];

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
    direnv = {
      enable = true;
      nix-direnv.enable = true;
    };
  };
}
