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
        --suffix PATH : ${lib.makeBinPath [ pkgs.git ]} \
        --set-default CLAUDE_WIKI_SESSIONS_DIRS ${
          lib.escapeShellArg (lib.concatMapStringsSep ":" (dir: "${homeDir}/${dir}/projects") configDirs)
        }
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

  # Claude Code skips the idle notification when the user typed anything since the
  # turn ended, so a short wait stays quiet while they are at the terminal.
  idleNotifThresholdMs = 10000;

  homeDir = config.home.homeDirectory;
  wikiDir = "${config.xdg.dataHome}/claude-wiki";

  # One Claude Code config directory per account: `claude` uses ~/.claude, and the
  # `sclaude` and `tclaude` fish functions set CLAUDE_CONFIG_DIR to ~/.sclaude and
  # ~/.tclaude. All of them get the same rules, skills, and settings; their `projects`
  # directories link to ~/.claude/projects, so transcripts and auto memory are shared.
  configDirs = [
    ".claude"
    ".sclaude"
    ".tclaude"
  ];
  extraConfigDirs = lib.remove ".claude" configDirs;
  sharedFiles = {
    # User-level rules rather than CLAUDE.md, which stays editable (/memory).
    "rules/claude-wiki.md" = ./rules/claude-wiki.md;
    "rules/nix.md" = ./rules/nix.md;
    "skills/claude-wiki" = ./skills/claude-wiki;
    "skills/codex-implement" = ./skills/codex-implement;
    # A plugin folder under skills/ loads as a mod (function hooks) in every session.
    "skills/wiki-mod" = ./wiki-mod;
  };
  codexScripts = ".claude/skills/codex-implement/scripts";

  # Merged into ~/.claude/settings.json on activation instead of linked from the store:
  # Claude Code writes that file itself (/config, /model, /permissions).
  settings = {
    # Transcripts are the only raw record of past sessions: `claude-wiki sessions`
    # searches them and the recall benchmark was mined from them. Claude Code deletes
    # them after 30 days unless told otherwise; they grow by roughly 25 GB a year.
    cleanupPeriodDays = 3650;
    # OSC 9 desktop notification plus a BEL; ghostty turns the bell into a sound and
    # a 🔔 on the tab (ghostty.nix). Inside tmux Claude Code wraps the OSC in a DCS
    # passthrough, which tmux.nix allows.
    preferredNotifChannel = "iterm2_with_bell";
    permissions.allow = [
      "Bash(claude-wiki *)"
      # a leading `//` marks an absolute path in permission rules
      "Read(/${wikiDir}/**)"
      "Edit(/${wikiDir}/**)"
      # codex-implement detaches its runner, follows it with codex-watch.sh from a watcher
      # subagent's Bash calls, and stops it with codex-stop.sh. Whether `~` is expanded
      # before matching is undocumented, so allow both spellings.
      "Bash(~/${codexScripts}/codex-monitor.sh *)"
      "Bash(${homeDir}/${codexScripts}/codex-monitor.sh *)"
      "Bash(~/${codexScripts}/codex-watch.sh *)"
      "Bash(${homeDir}/${codexScripts}/codex-watch.sh *)"
      "Bash(~/${codexScripts}/codex-stop.sh *)"
      "Bash(${homeDir}/${codexScripts}/codex-stop.sh *)"
      # codex-implement reads codex's JSON event log with jq
      "Bash(jq *)"
      # rules/nix.md has every session lint the Nix code it changes
      "Bash(statix check *)"
    ];
    # Keep the command string stable: array entries are unioned on merge, so a
    # changed command would be added next to the old one instead of replacing it.
    hooks = {
      SessionStart = [
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
      UserPromptSubmit = [
        {
          hooks = [
            {
              type = "command";
              command = "claude-wiki remind";
            }
          ];
        }
      ];
      # A failure's exact error text and a skill's tag identify useful knowledge;
      # ordinary calls and successful output mostly identify the topic already in hand.
      # Keep the command string stable: array entries are unioned on merge, so a
      # changed command would be added next to the old one instead of replacing it.
      PreToolUse = [
        {
          matcher = "Skill";
          hooks = [
            {
              type = "command";
              command = "claude-wiki recall";
              timeout = 5;
            }
          ];
        }
      ];
      PostToolUseFailure = [
        {
          hooks = [
            {
              type = "command";
              command = "claude-wiki recall";
              timeout = 5;
            }
          ];
        }
      ];
    };
  };
in
{
  home = {
    packages = [
      claude-wiki
      wiki-recap
    ];

    file =
      lib.mergeAttrsList (
        map (
          dir:
          lib.mapAttrs' (path: source: lib.nameValuePair "${dir}/${path}" { inherit source; }) sharedFiles
        ) configDirs
      )
      // lib.listToAttrs (
        map (
          dir:
          lib.nameValuePair "${dir}/projects" {
            source = config.lib.file.mkOutOfStoreSymlink "${homeDir}/.claude/projects";
          }
        ) extraConfigDirs
      );

    # Same approach as programs.zed-editor's mutableUserSettings, except that arrays
    # (permission rules, hooks) are unioned instead of replaced, so entries added at
    # runtime survive. Entries removed here are not removed from the file.
    activation.claudeCodeSettings = lib.hm.dag.entryAfter [ "linkGeneration" ] ''
      for settings in ${lib.escapeShellArgs (map (dir: "${homeDir}/${dir}/settings.json") configDirs)}; do
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
      done

      # The idle notification waits 60 s by default, and the key is read from the global
      # config (.claude.json, which CLAUDE_CONFIG_DIR moves), not from settings.json.
      # Written only when it differs: Claude Code rewrites this file all the time.
      for global in ${
        lib.escapeShellArgs (
          map (
            dir: if dir == ".claude" then "${homeDir}/.claude.json" else "${homeDir}/${dir}/.claude.json"
          ) configDirs
        )
      }; do
      [ -e "$global" ] || continue
      if [ "$(${lib.getExe pkgs.jq} '.messageIdleNotifThresholdMs' "$global")" != ${toString idleNotifThresholdMs} ]; then
        if updated="$(${lib.getExe pkgs.jq} '.messageIdleNotifThresholdMs = ${toString idleNotifThresholdMs}' "$global")"; then
          printf '%s\n' "$updated" > "$global"
        else
          warnEcho "$global is not valid JSON; not setting messageIdleNotifThresholdMs"
        fi
      fi
      done
    '';
  };

  programs.fish.functions = {
    sclaude = {
      description = "Claude Code with the second account (config directory ~/.sclaude)";
      wraps = "claude";
      body = "env CLAUDE_CONFIG_DIR=$HOME/.sclaude claude $argv";
    };
    tclaude = {
      description = "Claude Code with the third account (config directory ~/.tclaude)";
      wraps = "claude";
      body = "env CLAUDE_CONFIG_DIR=$HOME/.tclaude claude $argv";
    };
    # Nothing is shared with ~/.codex: the second account is only there for when the
    # first one runs out of usage. codex-implement reaches it with --account codex1.
    codex1 = {
      description = "Codex CLI with the second account (CODEX_HOME ~/.codex1)";
      wraps = "codex";
      body = "env CODEX_HOME=$HOME/.codex1 codex $argv";
    };
  };
}
