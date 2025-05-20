{ inputs, ... }:
{
  additions = final: _prev: import ../packages final.pkgs;

  modifications = final: prev: {
    opentrack = prev.opentrack.overrideAttrs (old: {
      version = "2024.1.1";
      src = final.fetchFromGitHub {
        owner = "opentrack";
        repo = "opentrack";
        rev = "opentrack-2024.1.1";
        hash = "sha256-IMhPvOBeJoLE+vg0rsKGs8Vhbpse8bIh0DeOwBubOUw=";
      };
      patches = [ ];
      nativeBuildInputs =
        old.nativeBuildInputs
        ++ (with final; [
          wine64Packages.base
          pkgsi686Linux.glibc
          onnxruntime
        ]);
      cmakeFlags = old.cmakeFlags ++ [
        "-DSDK_WINE=ON"
        "-DONNXRuntime_INCLUDE_DIR=${final.onnxruntime.dev}/include"
      ];
      desktopItems = [
        (final.makeDesktopItem rec {
          name = "opentrack";
          exec = "opentrack -platform xcb";
          icon = final.fetchurl {
            url = "https://github.com/opentrack/opentrack/raw/opentrack-2024.1.1/gui/images/opentrack.png";
            sha256 = "0d114zk78f7nnrk89mz4gqn7yk3k71riikdn29w6sx99h57f6kgn";
          };
          desktopName = name;
          genericName = "Head tracking software";
          categories = [ "Utility" ];
        })
      ];
    });

    vscode = final.symlinkJoin {
      name = "vscode";
      paths = [ prev.vscode ];
      buildInputs = [ final.makeWrapper ];
      postBuild = ''
        wrapProgram $out/bin/code \
          --add-flags "--enable-wayland-ime"
      '';
    };

    # desktop entry for slack directly refers to `$out/bin` so we cannot use symlinkJoin
    # re-compilation of slack package is not a big of deal (because it just unpacks release binary) so just overrideAttrs
    slack = prev.slack.overrideAttrs (old: {
      installPhase =
        builtins.replaceStrings
          [
            "--ozone-platform-hint=auto"
          ]
          [
            "--ozone-platform-hint=auto --enable-wayland-ime"
          ]
          old.installPhase;
    });
  };

  unstable-packages = final: _prev: {
    unstable = import inputs.nixpkgs-unstable {
      inherit (final) system;
      config.allowUnfree = true;
    };
  };
}
