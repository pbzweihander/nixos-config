{
  config,
  lib,
  osConfig,
  pkgs,
  ...
}:
let
  # The host the remote launcher connects to; on that host itself it makes no sense,
  # so neither the extra config nor the desktop entry is installed there.
  remoteHost = "rossmann";
  isRemoteHost = osConfig.networking.hostName == remoteHost;

  # Its own application id, so that the window does not land under the plain ghostty
  # entry in the taskbar, and a copy of the stock icon with the screen hue-rotated
  # from blue to amber, so the two are told apart at a glance. 197 is the hue: 50
  # gives green and 160 magenta.
  remoteClass = "com.mitchellh.ghostty.${remoteHost}";
  remoteIcon = pkgs.runCommand "${remoteClass}-icon" { nativeBuildInputs = [ pkgs.imagemagick ]; } ''
    for size in 16 32 128 256 512 1024; do
      dir="$out/share/icons/hicolor/''${size}x''${size}/apps"
      mkdir -p "$dir"
      magick "${config.programs.ghostty.package}/share/icons/hicolor/''${size}x''${size}/apps/com.mitchellh.ghostty.png" \
        -modulate 100,120,197 "$dir/${remoteClass}.png"
    done
  '';
in
{
  # `ghostty --config-file=.../ghostty/remote` starts a window whose *every* new tab
  # and split runs this command, so a remote session does not have to be started by
  # hand in each one. Loaded on top of the normal config. `tmux-view` comes from
  # ../modules/tmux.nix and attaches a per-tab view of one shared tmux session.
  home.packages = lib.optional (!isRemoteHost) remoteIcon;

  xdg.configFile = lib.optionalAttrs (!isRemoteHost) {
    "ghostty/remote".text = ''
      class = ${remoteClass}
      command = ssh -t ${remoteHost} tmux-view
    '';
  };

  # Named after the application id so that the taskbar matches the window to this
  # entry rather than to the plain ghostty one.
  xdg.desktopEntries = lib.optionalAttrs (!isRemoteHost) {
    ${remoteClass} = {
      name = "Ghostty (${remoteHost})";
      genericName = "Terminal";
      comment = "Ghostty window where every tab and split opens a tmux session on ${remoteHost}";
      exec = "ghostty --config-file=${config.xdg.configHome}/ghostty/remote --gtk-single-instance=false";
      icon = remoteClass;
      terminal = false;
      categories = [
        "System"
        "TerminalEmulator"
      ];
      settings.StartupWMClass = remoteClass;
    };
  };
}
