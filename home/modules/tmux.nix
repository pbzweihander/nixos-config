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

  # One session per terminal tab. A tab takes over the first session nobody is
  # attached to, which is the session left behind by a tab that was closed or by an
  # ssh connection that dropped, and starts a new one only when every session is
  # taken. Tabs therefore never show each other's screen, closing a tab leaves its
  # session and processes running for the next one, and exiting the last shell
  # destroys the session so the client exits and the terminal tab closes with it.
  home.packages = [
    (pkgs.writeShellScriptBin "tmux-view" ''
      T=${pkgs.tmux}/bin/tmux

      sid=$($T list-sessions -F '#{session_attached} #{session_id}' 2>/dev/null \
             | awk '$1 == 0 { print $2; exit }')
      if [ -n "$sid" ]; then
        exec $T attach-session -t "$sid"
      else
        exec $T new-session
      fi
    '')
  ];
}
