{
  programs = {
    git = {
      enable = true;
      settings = {
        advice.skippedCherryPicks = false;
        user = {
          email = "pbzweihander@protonmail.com";
          name = "pbzweihander";
        };
        branch.autosetuprebase = "always";
        color.ui = "auto";
        core = {
          editor = "hx";
          fscache = "yes";
          autocrlf = "no";
          quotepath = "no";
          precomposeunicode = "yes";
        };
        credential = {
          helper = "cache --timeout=3600";
          "https://github.com".username = "pbzweihander";
        };
        diff = {
          renames = "copies";
          tool = "vimdiff";
          compactionHeuristic = true;
        };
        difftool.prompt = "no";
        fetch.prune = "yes";
        gpg.format = "ssh";
        init.defaultBranch = "main";
        log.date = "iso8601";
        merge = {
          tool = "vimdiff";
          conflictstyle = "diff3";
        };
        pager = {
          reflog = "delta";
          stash = "false";
        };
        push.default = "simple";
        rebase.autoSquash = true;
      };
      ignores = [
        ".node-version"
        ".python-version"
        ".terraform-version"
        ".tool-versions"
        ".mise.toml"
        ".direnv"
        ".envrc"
        ".flake"
      ];
      signing = {
        key = "~/.ssh/id_ed25519.pub";
        signByDefault = true;
      };
    };
    delta = {
      enable = true;
      enableGitIntegration = true;
      enableJujutsuIntegration = true;
      options = {
        features = "decorations";
        navigate = true;
        side-by-side = true;

        pager = "less -+XF -Qc";

        minus-style = "syntax red";
        plus-style = "syntax green";
      };
    };
  };
}
