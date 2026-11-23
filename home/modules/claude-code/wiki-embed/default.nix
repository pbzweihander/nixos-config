# The semantic retrieval daemon and its login service. Import it on the machines that
# should have them; unlike agent-wiki this is not wanted everywhere. It pulls in a
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
      Description = "Resident semantic retrieval for agent-wiki";
      Wants = [ "default.target" ];
      After = [ "default.target" ];
    };
    Service = {
      ExecStart = lib.getExe wiki-embed;
      Environment = [
        "HF_HUB_OFFLINE=1"
        "HF_HOME=${config.xdg.cacheHome}/huggingface"
        # Rerank logits rose by about one point when the wiki switched to Korean
        # (2026-11-19): with Korean prompts against Korean pages the median score of an
        # answerable prompt went +0.71 -> +1.85 and of a near miss -1.49 -> -0.93. The
        # English-era cutoff -1.25 (useful 57/74, 39 misfires over 86 unanswerable
        # queries on the 1387-page wiki) would now misfire 51 times. -0.25 is better on
        # both counts: useful 63/74, 33 misfires. The benchmark's unanswerable set
        # predates some pages that now answer it, so misfires are an upper bound.
        "WIKI_EMBED_RERANK_THRESHOLD=-0.25"
      ];
      Restart = "on-failure";
      RestartSec = 30;
      Nice = 10;
    };
    Install.WantedBy = [ "default.target" ];
  };
}
