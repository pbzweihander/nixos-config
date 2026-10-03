{
  config,
  lib,
  pkgs,
  ...
}:
let
  wiki-curator = pkgs.callPackage ./package.nix { };
in
{
  home.packages = [ wiki-curator ];

  systemd.user = {
    services.wiki-curator = {
      Unit.Description = "Curate claude-wiki with preservation and Codex gates";
      Service = {
        Type = "oneshot";
        ExecStart = "${lib.getExe wiki-curator} run";
        Environment = [
          "PATH=%h/.local/bin:${config.home.profileDirectory}/bin:/run/current-system/sw/bin"
          "XDG_CACHE_HOME=${config.xdg.cacheHome}"
        ];
        Nice = 10;
        TimeoutStartSec = "2h";
      };
    };
    timers.wiki-curator = {
      Unit.Description = "Daily claude-wiki curation";
      Timer = {
        OnCalendar = "*-*-* 05:00:00";
        Persistent = true;
        RandomizedDelaySec = "15min";
      };
      Install.WantedBy = [ "timers.target" ];
    };
  };
}
