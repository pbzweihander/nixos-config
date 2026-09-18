{ pkgs, ... }:
{
  programs.tmux = {
    enable = true;
    terminal = "tmux-256color";
    escapeTime = 10;
    historyLimit = 50000;
    mouse = true;
    extraConfig = ''
      # ghostty installs its own terminfo on remote hosts (the ssh-terminfo shell
      # integration feature), so tell tmux the outer terminal does 24-bit colour.
      set -as terminal-features ",xterm-ghostty:RGB"
    '';
  };

  # Attaches a throwaway view to the shared `main` session. Sessions in a group share
  # their window list but keep their own current window, so every ghostty tab or split
  # can look at a different window of one session. `destroy-unattached` applies to the
  # view only, so closing a tab leaves `main` and its processes running.
  home.packages = [
    (pkgs.writeShellScriptBin "tmux-view" ''
      exec ${pkgs.tmux}/bin/tmux \
        new-session -Ad -s main \; \
        new-session -t main \; \
        set-option destroy-unattached on
    '')
  ];
}
