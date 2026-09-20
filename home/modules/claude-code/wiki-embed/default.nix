# The semantic retrieval daemon and its login service. Import it on the machines that
# should have them; unlike claude-wiki this is not wanted everywhere. It pulls in a
# ROCm build of torch, some 15 GB of closure, and it is only useful on a machine with
# a GPU the daemon may borrow. Where it is absent the prompt hook simply suggests
# nothing, which is the intended behaviour rather than a degraded one.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  wiki-embed = pkgs.callPackage ./package.nix { };
in
{
  home.packages = [ wiki-embed ];

  systemd.user.services.wiki-embed = {
    Unit = {
      Description = "Resident semantic retrieval for claude-wiki";
      Wants = [ "default.target" ];
      After = [ "default.target" ];
    };
    Service = {
      ExecStart = lib.getExe wiki-embed;
      Environment = [
        "HF_HUB_OFFLINE=1"
        "HF_HOME=${config.xdg.cacheHome}/huggingface"
      ];
      Restart = "on-failure";
      RestartSec = 30;
      Nice = 10;
    };
    Install.WantedBy = [ "default.target" ];
  };
}
