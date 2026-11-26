{
  config,
  lib,
  multiverse,
  pkgs,
  ...
}:
let
  agent-wiki = pkgs.rustPlatform.buildRustPackage {
    pname = "agent-wiki";
    version = "0.1.0";
    src = ./agent-wiki;
    cargoLock.lockFile = ./agent-wiki/Cargo.lock;
    nativeBuildInputs = [ pkgs.makeWrapper ];
    nativeCheckInputs = [
      pkgs.clippy
      pkgs.rustfmt
      pkgs.util-linux
    ];
    preCheck = ''
      cargo fmt --check
      cargo clippy --offline --all-targets -- -D warnings
    '';
    # Legacy defaults preserve explicit settings under either environment name.
    postInstall = ''
      wrapProgram $out/bin/agent-wiki \
        --suffix PATH : ${
          lib.makeBinPath [
            pkgs.git
            pkgs.util-linux
          ]
        } \
        --run ${lib.escapeShellArg ''
          if [ -z "''${AGENT_WIKI_CODEX_SESSIONS_DIRS+x}''${CLAUDE_WIKI_CODEX_SESSIONS_DIRS+x}" ]; then
            export AGENT_WIKI_CODEX_SESSIONS_DIRS="''${CODEX_HOME:+$CODEX_HOME/sessions:}${homeDir}/.codex/sessions:${homeDir}/.pcodex/sessions"
          fi
          if [ -z "''${AGENT_WIKI_SESSIONS_DIRS+x}''${CLAUDE_WIKI_SESSIONS_DIRS+x}" ]; then
            export AGENT_WIKI_SESSIONS_DIRS="''${CLAUDE_CONFIG_DIR:+$CLAUDE_CONFIG_DIR/projects:}${
              lib.concatMapStringsSep ":" (dir: "${homeDir}/${dir}/projects") configDirs
            }"
          fi
        ''}
      ln -s agent-wiki $out/bin/claude-wiki
    '';
    doInstallCheck = true;
    nativeInstallCheckInputs = [ pkgs.git ];
    installCheckPhase = ''
      runHook preInstallCheck
      bash smoke-test.sh $out/bin/agent-wiki
      test "$(readlink $out/bin/claude-wiki)" = agent-wiki
      sed 's/AGENT_WIKI_/CLAUDE_WIKI_/g' smoke-test.sh > smoke-test-legacy.sh
      bash smoke-test-legacy.sh $out/bin/claude-wiki
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
              agent-wiki
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
  wikiDir = "${config.xdg.dataHome}/agent-wiki";
  legacyWikiDir = "${config.xdg.dataHome}/claude-wiki";

  # One Claude Code config directory per account: `claude` uses ~/.claude, and the
  # `pclaude` fish function sets CLAUDE_CONFIG_DIR to ~/.pclaude. All of them get the
  # same settings; their `projects`, `skills`, `rules`, and `plugins` directories
  # link to ~/.claude, so transcripts, auto memory, skills (including ones written
  # at runtime), rules, and installed plugins are shared. Which plugins are enabled
  # stays per account, in each settings.json. claude.ai-synced skills and plugins live
  # in per-account buckets (skills/synced/<org>_<account>, plugins/synced/...), so the
  # accounts coexist there.
  configDirs = [
    ".claude"
    ".pclaude"
  ];
  extraConfigDirs = lib.remove ".claude" configDirs;
  codexConfigDirs = [
    ".codex"
    ".pcodex"
  ];
  codexHookPython = pkgs.python3.withPackages (ps: [ ps.tomlkit ]);
  antigravityHookMerge =
    pkgs.runCommand "agent-wiki-antigravity-hooks" { nativeBuildInputs = [ pkgs.python3 ]; }
      ''
        cp ${./antigravity-hooks}/*.py .
        python3 -m unittest discover -v
        install -Dm644 merge.py $out/libexec/merge.py
      '';
  sharedDirs = [
    "projects"
    "skills"
    "rules"
    "plugins"
  ];
  # Linked into ~/.claude only; the other accounts reach them through their
  # `sharedDirs` links.
  claudeFiles = {
    # User-level rules rather than CLAUDE.md, which stays editable (/memory).
    "rules/agent-wiki.md" = ./rules/agent-wiki.md;
    "rules/nix.md" = ./rules/nix.md;
    "rules/rust.md" = ./rules/rust.md;
    # One directory only: a second name for the same skill would list it twice; its
    # description keeps "claude-wiki" as a trigger phrase.
    "skills/agent-wiki" = ./skills/agent-wiki;
    # Not installed for now: implementation is delegated to Codex workers through Orca
    # instead. The source stays in ./skills/codex-implement; uncomment to bring it back.
    # "skills/codex-implement" = ./skills/codex-implement;
    # A plugin folder under skills/ loads as a mod (function hooks) in every session.
    "skills/wiki-mod" = ./wiki-mod;
  };
  codexScripts = ".claude/skills/codex-implement/scripts";

  # Merged into ~/.claude/settings.json on activation instead of linked from the store:
  # Claude Code writes that file itself (/config, /model, /permissions).
  settings = {
    # Transcripts are the only raw record of past sessions: `agent-wiki sessions`
    # searches them and the recall benchmark was mined from them. Claude Code deletes
    # them after 30 days unless told otherwise; they grow by roughly 25 GB a year.
    cleanupPeriodDays = 3650;
    # OSC 9 desktop notification plus a BEL; ghostty turns the bell into a sound and
    # a 🔔 on the tab (ghostty.nix). Inside tmux Claude Code wraps the OSC in a DCS
    # passthrough, which tmux.nix allows.
    preferredNotifChannel = "iterm2_with_bell";
    permissions.allow = [
      "Bash(agent-wiki *)"
      "Bash(claude-wiki *)"
      "Read(/${legacyWikiDir}/**)"
      "Edit(/${legacyWikiDir}/**)"
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
      SessionEnd = [
        {
          hooks = [
            {
              type = "command";
              command = "agent-wiki record";
              timeout = 5;
            }
          ];
        }
      ];
      SessionStart = [
        {
          hooks = [
            {
              type = "command";
              command = "agent-wiki context --hook";
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
              command = "agent-wiki remind --hook";
            }
          ];
        }
      ];
      # Event reminders share the interval counter; keep these command strings
      # stable because activation unions hook entries. Event processing fails open.
      PostToolUse = [
        {
          matcher = "Bash";
          hooks = [
            {
              type = "command";
              command = "agent-wiki remind --event";
              timeout = 2;
            }
          ];
        }
        {
          matcher = "Bash";
          hooks = [
            {
              type = "command";
              command = "agent-wiki recall --hook";
              timeout = 5;
            }
          ];
        }
      ];
      PreCompact = [
        {
          hooks = [
            {
              type = "command";
              command = "agent-wiki remind --event";
              timeout = 2;
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
              command = "agent-wiki recall --hook";
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
              command = "agent-wiki recall --hook";
              timeout = 5;
            }
          ];
        }
        # A separate entry preserves the existing recall entry during array union.
        {
          hooks = [
            {
              type = "command";
              command = "agent-wiki remind --event";
              timeout = 2;
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
      agent-wiki
      wiki-recap
    ];

    file =
      lib.mapAttrs' (path: source: lib.nameValuePair ".claude/${path}" { inherit source; }) claudeFiles
      // {
        ".agents/skills/agent-wiki".source = ./skills/agent-wiki;
        ".gemini/config/skills/agent-wiki".source = ./skills/agent-wiki;
      }
      // lib.listToAttrs (
        lib.concatMap (
          dir:
          map (
            sub:
            lib.nameValuePair "${dir}/${sub}" {
              source = config.lib.file.mkOutOfStoreSymlink "${homeDir}/.claude/${sub}";
            }
          ) sharedDirs
        ) extraConfigDirs
      );

    # Before `skills` became a link, each extra config directory had its own skills
    # directory holding Nix-managed links and that account's claude.ai-synced bucket.
    # Move the account's own content into ~/.claude/skills and drop the old
    # directory, so the link can take its place. Anything that would collide stays,
    # and checkLinkTargets then moves the directory aside with the backup extension.
    activation = {
      # Antigravity owns this mutable file. Replace only our named hook entry.
      antigravityWikiHooks = lib.hm.dag.entryAfter [ "linkGeneration" ] ''
        run ${lib.getExe pkgs.python3} ${antigravityHookMerge}/libexec/merge.py \
          --wiki ${lib.getExe' agent-wiki "agent-wiki"} \
          ${lib.escapeShellArg "${homeDir}/.gemini/config/hooks.json"}
      '';

      claudeCodeSharedSkills = lib.hm.dag.entryBefore [ "checkLinkTargets" ] ''
        shared=${lib.escapeShellArg "${homeDir}/.claude/skills"}
        for skills in ${lib.escapeShellArgs (map (dir: "${homeDir}/${dir}/skills") extraConfigDirs)}; do
        [ -d "$skills" ] && [ ! -L "$skills" ] || continue
        run mkdir -p "$shared/synced"
        if [ -d "$skills/synced" ]; then
          for entry in "$skills/synced"/* "$skills/synced"/.bucket-*; do
            [ -e "$entry" ] || continue
            [ -e "$shared/synced/''${entry##*/}" ] || run mv "$entry" "$shared/synced/"
          done
          run rmdir --ignore-fail-on-non-empty "$skills/synced"
        fi
        for entry in "$skills"/* "$skills"/.[!.]*; do
          [ -e "$entry" ] || [ -L "$entry" ] || continue
          case "$(readlink "$entry")" in
            /nix/store/*) run rm "$entry" ;;
            *) [ -e "$shared/''${entry##*/}" ] || run mv "$entry" "$shared/" ;;
          esac
        done
        run rmdir --ignore-fail-on-non-empty "$skills"
        done
      '';

      # Old symlinks keep in-flight sessions and curator git worktrees usable.
      # New service packages restart normally on Home Manager activation.
      agentWikiMigration = lib.hm.dag.entryBetween [ "linkGeneration" ] [ "writeBoundary" ] ''
        run ${lib.getExe pkgs.python3} ${./migrate-wiki.py} --mv ${lib.getExe' pkgs.coreutils "mv"} \
          ${lib.escapeShellArg config.xdg.dataHome} \
          ${lib.escapeShellArg config.xdg.stateHome} \
          ${lib.escapeShellArg config.xdg.cacheHome} \
          "''${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
      '';

      # Same approach as programs.zed-editor's mutableUserSettings, except that arrays
      # (permission rules, hooks) are unioned instead of replaced, so entries added at
      # runtime survive. Only exact legacy wiki hook commands are removed.
      claudeCodeSettings = lib.hm.dag.entryAfter [ "linkGeneration" ] ''
        for settings in ${lib.escapeShellArgs (map (dir: "${homeDir}/${dir}/settings.json") configDirs)}; do
        if [ -n "''${DRY_RUN_CMD:-}" ] || [ "''${DRY_RUN:-0}" = 1 ]; then
          verboseEcho "Would merge $settings"
          continue
        fi
        [ ! -L "$settings" ] || { warnEcho "Refusing symlink $settings"; continue; }
        dir="$(dirname "$settings")"
        run mkdir -p "$dir"
        (
          set -o pipefail
          tmp="$(mktemp "$dir/.settings.json.XXXXXX")" || exit 1
          trap 'rm -f "$tmp"' EXIT
          input="$settings"
          if [ ! -e "$input" ]; then input=/dev/null; fi
          before="$(sha256sum "$input")"
          if ${lib.getExe pkgs.jq} -s 'if length == 0 then {} elif length == 1 and (.[0] | type) == "object" then .[0] else error("expected one settings object") end' "$input" |
            ${lib.getExe pkgs.jq} --slurpfile static ${
              (pkgs.formats.json { }).generate "claude-code-settings.json" settings
            } -f ${./merge-settings.jq} > "$tmp" &&
            [ -s "$tmp" ] && ${lib.getExe pkgs.jq} -e 'type == "object"' "$tmp" > /dev/null &&
            [ ! -L "$settings" ] && { [ "$input" != /dev/null ] || [ ! -e "$settings" ]; } && [ "$before" = "$(sha256sum "$input")" ]; then
            chmod 600 "$tmp"
            run mv -f "$tmp" "$settings"
          else
            warnEcho "$settings changed or is invalid; not merging Claude Code settings"
          fi
        )
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
        [ -e "$global" ] && [ ! -L "$global" ] || continue
        if [ -n "''${DRY_RUN_CMD:-}" ] || [ "''${DRY_RUN:-0}" = 1 ]; then continue; fi
        if [ "$(${lib.getExe pkgs.jq} '.messageIdleNotifThresholdMs' "$global")" != ${toString idleNotifThresholdMs} ]; then
          (
            tmp="$(mktemp "$(dirname "$global")/.settings.json.XXXXXX")" || exit 1
            trap 'rm -f "$tmp"' EXIT
            before="$(sha256sum "$global")"
            if ${lib.getExe pkgs.jq} '.messageIdleNotifThresholdMs = ${toString idleNotifThresholdMs}' "$global" > "$tmp" &&
              [ -s "$tmp" ] && ${lib.getExe pkgs.jq} -e 'type == "object"' "$tmp" > /dev/null &&
              [ ! -L "$global" ] && [ "$before" = "$(sha256sum "$global")" ]; then
              chmod 600 "$tmp"
              run mv -f "$tmp" "$global"
            else
              warnEcho "$global changed or is invalid; not setting messageIdleNotifThresholdMs"
            fi
          )
        fi
        done
        # Orca and Codex own these mutable files. Merge only marked wiki hooks and
        # their trust state; preserve foreign array positions and AGENTS.md content.
        run ${lib.getExe codexHookPython} ${./codex-hooks/merge.py} \
          --codex ${lib.getExe multiverse.instance.tip.codex} \
          --wiki ${lib.getExe' agent-wiki "agent-wiki"} \
          --rules ${./rules/agent-wiki.md} \
          ${lib.escapeShellArgs (map (dir: "${homeDir}/${dir}") codexConfigDirs)}
      '';
    };
  };

  programs.fish.functions = {
    pclaude = {
      description = "Claude Code with config directory ~/.pclaude";
      wraps = "claude";
      body = "env CLAUDE_CONFIG_DIR=$HOME/.pclaude claude $argv";
    };
    # Nothing is shared with ~/.codex. codex-implement reaches it with --account pcodex.
    pcodex = {
      description = "Codex CLI with CODEX_HOME ~/.pcodex";
      wraps = "codex";
      body = "env CODEX_HOME=$HOME/.pcodex codex $argv";
    };
  };
}
