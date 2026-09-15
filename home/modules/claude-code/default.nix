{
  config,
  lib,
  pkgs,
  ...
}:
let
  claude-wiki =
    pkgs.runCommand "claude-wiki"
      {
        nativeBuildInputs = [
          pkgs.makeWrapper
          pkgs.ruff
          pkgs.git
        ];
      }
      ''
        ruff check --no-cache --select E9,F ${./claude-wiki/claude-wiki.py}
        install -Dm755 ${./claude-wiki/claude-wiki.py} $out/libexec/claude-wiki.py
        makeWrapper ${pkgs.python3.withPackages (ps: [ ps.pyyaml ])}/bin/python3 $out/bin/claude-wiki \
          --add-flags $out/libexec/claude-wiki.py \
          --suffix PATH : ${lib.makeBinPath [ pkgs.git ]}
        bash ${./claude-wiki/smoke-test.sh} $out/bin/claude-wiki
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
  };
in
{
  home.packages = [ claude-wiki ];

  home.file = {
    # A user-level rule rather than ~/.claude/CLAUDE.md, which stays editable (/memory).
    ".claude/rules/claude-wiki.md".source = ./rules/claude-wiki.md;
    ".claude/skills/claude-wiki".source = ./skills/claude-wiki;
    ".claude/skills/codex-implement".source = ./skills/codex-implement;
  };

  # Same approach as programs.zed-editor's mutableUserSettings, except that arrays
  # (permission rules, hooks) are unioned instead of replaced, so entries added at
  # runtime survive. Entries removed here are not removed from the file.
  home.activation.claudeCodeSettings = lib.hm.dag.entryAfter [ "linkGeneration" ] ''
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
}
