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

      sessions=$($T list-sessions -F '#{session_attached} #{session_id} #{session_name}' 2>/dev/null)

      sid=$(printf '%s\n' "$sessions" | awk '$1 == 0 { print $2; exit }')
      if [ -n "$sid" ]; then
        exec $T attach-session -t "$sid"
      fi

      # tmux's own default name is a counter that never reuses a number, so a tab
      # closed and reopened a few times ends up called something like 17. Take the
      # lowest free number instead, which keeps the names as small as the tab count.
      n=0
      while printf '%s\n' "$sessions" | awk '{ print $3 }' | grep -qx "$n"; do
        n=$((n + 1))
      done
      exec $T new-session -s "$n"
    '')
  ];
}
