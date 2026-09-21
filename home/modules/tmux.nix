{ pkgs, ... }:
{
  programs.tmux = {
    enable = true;
    terminal = "tmux-256color";
    escapeTime = 10;
    # Off by default in tmux, and home-manager writes that default out explicitly.
    # Claude Code warns when it is off, since without the focus in and out reports it
    # cannot tell whether its terminal is in front.
    focusEvents = true;
    historyLimit = 50000;
    mouse = true;
    # C-b is page-up in less, man and vim, so tmux holding it makes paging over ssh
    # annoying. C-a is the usual replacement but it is beginning-of-line in fish, which
    # trades a pager conflict for a shell one. C-Space is free in all of them.
    prefix = "C-Space";
    keyMode = "vi";
    extraConfig = ''
      # ghostty installs its own terminfo on remote hosts (the ssh-terminfo shell
      # integration feature), so tell tmux the outer terminal does 24-bit colour.
      set -as terminal-features ",xterm-ghostty:RGB"

      # tmux swallows the title the program inside it sets unless set-titles is on,
      # which is why a ghostty tab stops following the remote shell once tmux is in
      # the middle. #T is the pane title, which fish's own fish_title sets to the
      # host, the working directory, and the command it is about to run, so this
      # gives the same titles a plain ssh session would.
      set -g set-titles on
      set -g set-titles-string "#T"

      # tmux opens a new pane in the path tmux itself started in; keep the pane's own
      # directory instead, which is nearly always what the split is for.
      bind '"' split-window -c "#{pane_current_path}"
      bind % split-window -h -c "#{pane_current_path}"
      bind c new-window -c "#{pane_current_path}"

      # Copy with y as in vim, rather than emacs' Enter, and paste with p.
      bind -T copy-mode-vi v send -X begin-selection
      bind -T copy-mode-vi y send -X copy-selection-and-cancel
      bind p paste-buffer

      # tmux copies to the clipboard the moment a mouse selection ends, which
      # ghostty deliberately does not do (copy-on-select = false), so a drag meant
      # only to read something loses whatever was on the clipboard. Keep the
      # selection instead and wait for y; q or Escape leaves copy mode.
      # The double and triple click bindings are the stock ones with their trailing
      # copy-pipe-and-cancel dropped, and without -H so that copy mode is visible.
      bind -T copy-mode-vi MouseDragEnd1Pane send -X stop-selection
      bind -T root DoubleClick1Pane select-pane -t = \; \
        if -F "#{||:#{pane_in_mode},#{mouse_any_flag}}" { send -M } \
          { copy-mode ; send -X select-word }
      bind -T root TripleClick1Pane select-pane -t = \; \
        if -F "#{||:#{pane_in_mode},#{mouse_any_flag}}" { send -M } \
          { copy-mode ; send -X select-line }

      bind R source-file ~/.config/tmux/tmux.conf \; display "tmux.conf reloaded"
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
