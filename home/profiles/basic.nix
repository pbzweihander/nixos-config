{ config, ... }:
{
  imports = [
    ../modules/claude-code
    ../modules/fish
    ../modules/git.nix
    ../modules/helix
    ../modules/ssh.nix
    ../modules/terraform.nix
    ../modules/tmux.nix
    ../modules/yamlfmt
  ];

  home = {
    stateVersion = "26.05";
    username = "pbzweihander";
    homeDirectory = "/home/pbzweihander";
    # The nodejs prefix is in the read-only Nix store, so `npm i -g` installs into
    # ~/.local instead (~/.local/bin is already on PATH). npm-installed CLIs such as
    # gemini-cli then update themselves, like the native Claude Code build.
    sessionVariables.NPM_CONFIG_PREFIX = "${config.home.homeDirectory}/.local";
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
