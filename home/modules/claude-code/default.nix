{
  config,
  lib,
  pkgs,
  ...
}:
let
  claude-wiki = pkgs.rustPlatform.buildRustPackage {
    pname = "claude-wiki";
    version = "0.1.0";
    src = ./claude-wiki;
    cargoLock.lockFile = ./claude-wiki/Cargo.lock;
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postInstall = ''
      wrapProgram $out/bin/claude-wiki \
        --suffix PATH : ${lib.makeBinPath [ pkgs.git ]}
    '';
    doInstallCheck = true;
    nativeInstallCheckInputs = [ pkgs.git ];
    installCheckPhase = ''
      runHook preInstallCheck
      bash smoke-test.sh $out/bin/claude-wiki
      runHook postInstallCheck
    '';
  };

  # Runs without the user's fish config: the terminal greeting calls it on every new shell.
  wiki-recap =
    pkgs.runCommand "wiki-recap"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
      }
      ''
        install -Dm644 ${./wiki-recap/wiki-recap.fish} $out/libexec/wiki-recap.fish
        ${lib.getExe pkgs.fish} --no-config --no-execute $out/libexec/wiki-recap.fish
        makeWrapper ${lib.getExe pkgs.fish} $out/bin/wiki-recap \
          --add-flags "--no-config $out/libexec/wiki-recap.fish" \
          --suffix PATH : ${
            lib.makeBinPath [
              claude-wiki
              pkgs.coreutils
              pkgs.git
              pkgs.util-linux
            ]
          }
      '';

  wikiDir = "${config.xdg.dataHome}/claude-wiki";
  codexMonitor = ".claude/skills/codex-implement/scripts/codex-monitor.sh";

  # Merged into ~/.claude/settings.json on activation instead of linked from the store:
  # Claude Code writes that file itself (/config, /model, /permissions).
  settings = {
    permissions.allow = [
      "Bash(claude-wiki *)"
      # a leading `//` marks an absolute path in permission rules
      "Read(/${wikiDir}/**)"
      "Edit(/${wikiDir}/**)"
      # codex-implement starts this with the Monitor tool, which uses Bash rules. Whether
      # `~` is expanded before matching is undocumented, so allow both spellings.
      "Bash(~/${codexMonitor} *)"
      "Bash(${config.home.homeDirectory}/${codexMonitor} *)"
      # codex-implement reads codex's JSON event log with jq
      "Bash(jq *)"
      # rules/nix.md has every session lint the Nix code it changes
      "Bash(statix check *)"
    ];
    # Keep the command string stable: array entries are unioned on merge, so a
    # changed command would be added next to the old one instead of replacing it.
    hooks.SessionStart = [
      {
        hooks = [
          {
            type = "command";
            command = "claude-wiki context";
          }
        ];
      }
    ];
    # Keep the command string stable: array entries are unioned on merge, so a
    # changed command would be added next to the old one instead of replacing it.
    hooks.UserPromptSubmit = [
      {
        hooks = [
          {
            type = "command";
            command = "claude-wiki remind";
          }
        ];
      }
    ];
  };
in
{
  home = {
    packages = [
      claude-wiki
      wiki-recap
    ];

    file = {
      # User-level rules rather than ~/.claude/CLAUDE.md, which stays editable (/memory).
      ".claude/rules/claude-wiki.md".source = ./rules/claude-wiki.md;
      ".claude/rules/nix.md".source = ./rules/nix.md;
      ".claude/skills/claude-wiki".source = ./skills/claude-wiki;
      ".claude/skills/codex-implement".source = ./skills/codex-implement;
    };

    # Same approach as programs.zed-editor's mutableUserSettings, except that arrays
    # (permission rules, hooks) are unioned instead of replaced, so entries added at
    # runtime survive. Entries removed here are not removed from the file.
    activation.claudeCodeSettings = lib.hm.dag.entryAfter [ "linkGeneration" ] ''
      settings=${lib.escapeShellArg "${config.home.homeDirectory}/.claude/settings.json"}
      mkdir -p "$(dirname "$settings")"
      [ -e "$settings" ] || echo '{}' > "$settings"
      if merged="$(${lib.getExe pkgs.jq} --slurpfile static ${
        (pkgs.formats.json { }).generate "claude-code-settings.json" settings
      } '
        def merge($b):
          if type == "object" and ($b | type) == "object" then
            reduce ($b | keys_unsorted[]) as $k (.; .[$k] |= merge($b[$k]))
          elif type == "array" and ($b | type) == "array" then
            reduce $b[] as $x (.; if index([$x]) then . else . + [$x] end)
          else $b end;
        merge($static[0])
      ' "$settings")"; then
        printf '%s\n' "$merged" > "$settings"
      else
        warnEcho "$settings is not valid JSON; not merging Claude Code settings"
      fi
    '';
  };
}
