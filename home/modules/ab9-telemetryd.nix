# Community MOZA AB9 telemetry-effects daemon (github.com/Specter242/dcs-linux-vr-ab9).
# It listens for DCS telemetry on 127.0.0.1:34400 and turns it into evdev force
# effects on the base. By default it runs dry-run: it logs the effects it would play
# without opening the base.
#
# `services.ab9-telemetryd.armed = true` enables motor output: it adds --apply, lifts
# PrivateDevices, and writes the arm file ~/.config/ab9-ffb/telemetry-effects-enabled
# with the exact phrase the daemon checks. The unit is skipped when that file is
# absent, so `rm` of the link plus a unit restart disarms until the next switch.
# ab9_profiled.py (persistent base writes over serial) is shipped only as a module
# that ab9_telemetryd.py imports; it has no command and no service here.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.ab9-telemetryd;
  armFile = "%h/.config/ab9-ffb/telemetry-effects-enabled";
  # Must match ARM_PHRASE in ab9_telemetryd.py.
  armPhrase = "I UNDERSTAND TELEMETRY EFFECTS MAY MOVE THE STICK";
  python = pkgs.python3.withPackages (ps: [ ps.evdev ]);

  ab9-ffb = pkgs.stdenvNoCC.mkDerivation {
    pname = "ab9-ffb";
    version = "0-unstable-2026-08-09";

    src = pkgs.fetchFromGitHub {
      owner = "Specter242";
      repo = "dcs-linux-vr-ab9";
      rev = "8e18313b90f62228dd6a516f82d0f6855c41e10f";
      hash = "sha256-QnD2hNQI00YqXY0iLlA3mQ/nA/UgemRSzymP4UXhFPY=";
    };

    nativeBuildInputs = [ python ];

    # MOZA Cockpit ships one Mirage F1 preset, `Mirage-F1CE`. The other flyable
    # variants (types from the module's entry.lua) fall back to it.
    postPatch = ''
      substituteInPlace ab9_profiled.py --replace-fail \
        '    "TF-51D": "P-51D-30-NA",' \
        '    "TF-51D": "P-51D-30-NA",
          "Mirage-F1EE": "Mirage-F1CE",
          "Mirage-F1BE": "Mirage-F1CE",'
    '';

    dontConfigure = true;
    dontBuild = true;

    # The tests use synthetic presets and never touch hardware.
    doCheck = true;
    checkPhase = ''
      runHook preCheck
      python3 -m unittest -v test_ab9_telemetry
      runHook postCheck
    '';

    installPhase = ''
      runHook preInstall
      # ab9_telemetryd.py imports ab9_profiled -> ab9_profile -> ab9_control -> moza.
      install -Dm644 -t $out/libexec/ab9-ffb \
        ab9_telemetryd.py ab9_profiled.py ab9_profile.py ab9_control.py moza.py
      install -Dm644 -t $out/share/ab9-ffb AB9ProfileExport.lua
      install -Dm644 -t $out/share/doc/ab9-ffb README.md PUBLIC_GUIDE.md LICENSE
      mkdir -p $out/bin
      cat > $out/bin/ab9-telemetryd <<EOF
      #!${pkgs.runtimeShell}
      exec ${python}/bin/python3 -I $out/libexec/ab9-ffb/ab9_telemetryd.py "\$@"
      EOF
      chmod +x $out/bin/ab9-telemetryd
      runHook postInstall
    '';

    meta = {
      description = "Synthesize MOZA AB9 telemetry effects from DCS (community prototype)";
      homepage = "https://github.com/Specter242/dcs-linux-vr-ab9";
      license = pkgs.lib.licenses.mit;
      mainProgram = "ab9-telemetryd";
    };
  };
in
{
  options.services.ab9-telemetryd = {
    armed = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Let the daemon drive the AB9 motor (--apply), and write its arm file
        ~/.config/ab9-ffb/telemetry-effects-enabled.
      '';
    };
    gain = lib.mkOption {
      type = lib.types.numbers.between 0.25 4.0;
      default = 1.0;
      example = 0.5;
      description = ''
        Multiplier for every synthesized effect's preset strength, applied before
        capPercent (--gain).
      '';
    };
    capPercent = lib.mkOption {
      type = lib.types.ints.between 5 50;
      default = 15;
      description = "Hard per-effect magnitude cap in percent (--cap-percent).";
    };
  };

  config = {
    home.packages = [ ab9-ffb ];

    # Loaded from DCS Saved Games/DCS/Scripts/Export.lua through Wine's Z: drive.
    xdg.configFile."ab9-ffb/telemetry-effects-enabled" = lib.mkIf cfg.armed {
      text = armPhrase + "\n";
    };

    xdg.dataFile."ab9-ffb/AB9ProfileExport.lua".source =
      "${ab9-ffb}/share/ab9-ffb/AB9ProfileExport.lua";

    systemd.user.services.ab9-telemetryd = {
      Unit = {
        Description = "MOZA AB9 telemetry effects from DCS${lib.optionalString (!cfg.armed) " (dry-run)"}";
        After = [ "graphical-session.target" ];
      }
      // lib.optionalAttrs cfg.armed {
        ConditionPathExists = armFile;
        # The daemon exits when the base is missing (e.g. passed to a VM); don't spin.
        StartLimitIntervalSec = 300;
        StartLimitBurst = 5;
      };
      Service = {
        Type = "simple";
        # Without --apply the daemon uses DryRunSink and never opens /dev/input.
        ExecStart = lib.concatStringsSep " " (
          [ (lib.getExe ab9-ffb) ]
          ++ lib.optional cfg.armed "--apply"
          ++ [
            "--gain ${toString cfg.gain}"
            "--cap-percent ${toString cfg.capPercent}"
          ]
        );
        Restart = "on-failure";
        RestartSec = if cfg.armed then 10 else 2;
        # Dry-run: no device nodes at all, so a stray --apply cannot reach the motor.
        PrivateDevices = !cfg.armed;
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_UNIX"
        ];
      };
      Install.WantedBy = [ "default.target" ];
    };
  };
}
