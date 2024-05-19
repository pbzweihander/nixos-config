{
  programs.git = {
    enable = true;
    delta = {
      enable = true;
      options = {
        features = "decorations";
        pager = "less -+XF -Qc";
        side-by-side = true;
        plus-style = "syntax green";
        minus-style = "syntax red";
      };
    };
    extraConfig = {
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
        diff = "delta";
        log = "delta";
        reflog = "delta";
        show = "delta";
        stash = "false";
      };
      push.default = "simple";
    };
    ignores = [
      ".node-version"
      ".python-version"
      ".terraform-version"
      ".tool-versions"
      ".mise.toml"
    ];
    signing = {
      key = "~/.ssh/id_ed25519.pub";
      signByDefault = true;
    };
    userEmail = "pbzweihander@protonmail.com";
    userName = "pbzweihander";
  };
}

