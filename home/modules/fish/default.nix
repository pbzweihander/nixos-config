{ pkgs, ... }:
{
  programs = {
    fish = {
      enable = true;
      shellInit = "set -gx fish_greeting";
      plugins = [
        {
          name = "bass";
          src = pkgs.fetchFromGitHub {
            owner = "edc";
            repo = "bass";
            rev = "79b62958ecf4e87334f24d6743e5766475bcf4d0";
            hash = "sha256-3d/qL+hovNA4VMWZ0n1L+dSM1lcz7P5CQJyy+/8exTc=";
          };
        }
        {
          name = "kubectl-aliases";
          src = pkgs.fetchFromGitHub {
            owner = "pbzweihander";
            repo = "kubectl-aliases-fish";
            rev = "84dd470ade638b572787c6ef3902507874e7a897";
            hash = "sha256-f4Aom259FpooAIxldDKooAXf3yRHRebbFuMNsM8wX2A=";
          };
        }
        {
          name = "truck";
          src = pkgs.fetchFromGitHub {
            owner = "pbzweihander";
            repo = "truck";
            rev = "6ab72827872e2762a6722d2f97c5b5bf1371ccef";
            hash = "sha256-+N7yk3Y5ikEvuCWQ4rKqSWuqx8ZGxMyC7qytTqfR14w=";
          };
        }
        {
          name = "git-ignore";
          src = pkgs.fetchFromGitHub {
            owner = "pbzweihander";
            repo = "fish-git-ignore";
            rev = "1b677b71d4c28847f7db7b35554a8acedeb2f1ff";
            hash = "sha256-CTb7aI/Ahu0bjfyCvZlHqTRgCtvVdZnjcIJmu+XMNO0=";
          };
        }
        {
          name = "git";
          src = pkgs.fetchFromGitHub {
            owner = "pbzweihander";
            repo = "fish-plugin-git";
            rev = "ed55f74105f115035e77c8f13919926613da3f4b";
            hash = "sha256-xlrOZ/uM/eeLm9nNAuoQkTrCMbdNQ9rn1qdg0sp/EP8=";
          };
        }
        {
          name = "fzf";
          src = pkgs.fetchFromGitHub {
            owner = "PatrickF1";
            repo = "fzf.fish";
            rev = "8920367cf85eee5218cc25a11e209d46e2591e7a";
            hash = "sha256-T8KYLA/r/gOKvAivKRoeqIwE2pINlxFQtZJHpOy9GMM=";
          };
        }
      ];
    };
    starship.enable = true;
    zoxide.enable = true;
  };

  home.file = {
    ".config/fish/conf.d/aliases.fish".source = ./conf.d/aliases.fish;
    ".config/fish/conf.d/programs.fish".source = ./conf.d/programs.fish;
    ".config/fish/conf.d/vi.fish".source = ./conf.d/vi.fish;
    ".config/fish/functions/aws-ecr-login.fish".source = ./functions/aws-ecr-login.fish;
    ".config/fish/functions/ds.fish".source = ./functions/ds.fish;
    ".config/fish/functions/jp.fish".source = ./functions/jp.fish;
    ".config/fish/functions/kind-with-registry.fish".source = ./functions/kind-with-registry.fish;
    ".config/starship.toml".source = ./starship.toml;
  };
}
