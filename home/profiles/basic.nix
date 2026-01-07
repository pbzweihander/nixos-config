{
  imports = [
    ../modules/fish
    ../modules/git.nix
    ../modules/helix
    ../modules/ssh.nix
    ../modules/yamlfmt
  ];

  home = {
    stateVersion = "25.11";
    username = "pbzweihander";
    homeDirectory = "/home/pbzweihander";
  };

  fonts.fontconfig = {
    enable = true;
    defaultFonts = {
      serif = [ "Sarasa Gothic K" ];
      sansSerif = [ "Sarasa UI K" ];
      monospace = [ "Sarasa Mono K" ];
    };
  };

  xdg.enable = true;

  programs = {
    home-manager.enable = true;
    git.enable = true;
    vim.enable = true;
    helix.enable = true;
    nix-index.enableFishIntegration = false;
  };
}
