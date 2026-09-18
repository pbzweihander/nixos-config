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

  # Attaches a throwaway view to the shared `main` session and puts it on a window no
  # other view is showing, creating one when they are all taken. Sessions in a group
  # share their window list but keep their own current window, so every terminal tab
  # ends up on a different window without anything being typed, and a window freed by
  # a closed tab is picked up again by the next one. `destroy-unattached` applies to
  # the view only, so closing a tab leaves `main` and its processes running.
  home.packages = [
    (pkgs.writeShellScriptBin "tmux-view" ''
      T=${pkgs.tmux}/bin/tmux

      # Not `new-session -A`: on an existing session that attaches the client instead
      # of creating a view, and its -d is then read as attach-session's -d. The `=`
      # prefix keeps `main` from matching the `main-1`, `main-2`, ... views.
      $T has-session -t =main 2>/dev/null || $T new-session -d -s main

      # windows that another attached view is currently showing
      busy=$($T list-sessions -F '#{session_attached}:#{session_group}:#{window_index}' \
              | awk -F: '$1 != "0" && $2 == "main" { print $3 }')
      target=
      for w in $($T list-windows -t =main -F '#{window_index}'); do
        printf '%s\n' "$busy" | grep -qx "$w" || { target=$w; break; }
      done

      if [ -n "$target" ]; then
        exec $T new-session -t =main \; set-option destroy-unattached on \; select-window -t "$target"
      else
        exec $T new-session -t =main \; set-option destroy-unattached on \; new-window
      fi
    '')
  ];
}
