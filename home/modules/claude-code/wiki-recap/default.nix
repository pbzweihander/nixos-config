# The 6-hourly wiki-recap timer and the fish greeting that shows its brief. Import it
# on the machines that should have them; the wiki-recap command itself comes from
# ../default.nix on every machine.
{ config, ... }:
{
  # Every 6 hours, regenerate the wiki recap if the wiki changed (claude runs only then).
  # The brief it leaves is what new terminals show; see wiki-recap --help.
  systemd.user = {
    services.wiki-recap = {
      Unit = {
        Description = "Regenerate the claude-wiki recap";
        StartLimitIntervalSec = "2h";
        StartLimitBurst = 4;
      };
      Service = {
        Type = "oneshot";
        ExecStart = "${config.home.profileDirectory}/bin/wiki-recap --update";
        # Right after resume the network may not be up yet.
        Restart = "on-failure";
        RestartSec = "15min";
        TimeoutStartSec = "20min";
        Nice = 10;
      };
    };
    timers.wiki-recap = {
      Unit.Description = "Regenerate the claude-wiki recap every 6 hours";
      Timer = {
        OnCalendar = "*-*-* 03/6:00:00";
        Persistent = true;
        RandomizedDelaySec = "5min";
      };
      Install.WantedBy = [ "timers.target" ];
    };
  };

  programs.fish.functions.fish_greeting = "wiki-recap --brief";
}
