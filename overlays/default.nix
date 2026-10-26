{ inputs, lib }:
final: prev: {
  sddm-arona = prev.callPackage ./sddm-arona { };

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

  # Nested on KWin, gamescope < 3.16.31 attaches a buffer to its toplevel right
  # after a NULL-buffer unmap and gets disconnected for it ("attached a buffer
  # before configure event"). Backported instead of taking a newer gamescope
  # build, because Lutris puts this channel's glibc on LD_LIBRARY_PATH and a
  # binary linked against another glibc aborts at startup there.
  # Drop once this channel has gamescope >= 3.16.31.
  gamescope = prev.gamescope.overrideAttrs (old: {
    patches = (old.patches or [ ]) ++ [
      (final.fetchpatch {
        url = "https://github.com/ValveSoftware/gamescope/commit/c50ddfa9b71a.patch";
        hash = "sha256-w0a97antZIXk3P+XWCKYqEk9JdZ40nf8tjB48tXqoP8=";
      })
      (final.fetchpatch {
        url = "https://github.com/ValveSoftware/gamescope/commit/6867f509874f.patch";
        hash = "sha256-vPFquYCCII5NEDtNCTdfQqIbP5bKX+lTpj9GnAr2/WA=";
      })
    ];
  });

  spotify-qt = prev.spotify-qt.overrideAttrs (old: {
    version = "4.0.4";
    src = final.fetchFromGitHub {
      owner = "kraxarn";
      repo = "spotify-qt";
      rev = "a5eeff4fa0459d507d3f7b8583cf6f5a53c43540";
      hash = "sha256-5stDBpt+TJGihrmEVt0kB4Aiez7wIpqm4P6RnSDquxM=";
    };
  });
}
