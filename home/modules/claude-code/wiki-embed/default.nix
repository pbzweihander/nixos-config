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
        # The benchmark's own answer is +0.274, the lowest cutoff that rejects every
        # ordinary near miss, but it leaves the hook silent on 43% of the prompts the
        # wiki can answer. -1.25 is the knee of the trade-off measured on 74 answerable
        # and 86 unanswerable queries: useful rises 42 -> 58 while the pages named for a
        # question the wiki cannot answer rise 6 -> 27, and most of those 27 are from the
        # adversarial set, which names the right page for the wrong question. Past this
        # the curve flattens: -2.0 buys three more useful hits for nine more misfires.
        "WIKI_EMBED_RERANK_THRESHOLD=-1.25"
      ];
      Restart = "on-failure";
      RestartSec = 30;
      Nice = 10;
    };
    Install.WantedBy = [ "default.target" ];
  };
}
