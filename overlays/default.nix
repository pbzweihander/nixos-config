{ inputs, lib }:
final: prev: {
  sddm-arona = prev.callPackage ./sddm-arona { };

  helix-assist = prev.callPackage ./helix-assist { };

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

  spotify-qt = prev.spotify-qt.overrideAttrs (old: {
    version = "4.0.2-ac839e9";
    src = final.fetchFromGitHub {
      owner = "kraxarn";
      repo = "spotify-qt";
      rev = "ac839e947d8bec5c6a747448fe85228fbfd15c68";
      hash = "sha256-cCjAraRx2eo+oY2RXOa0qQVXNQtAFNhR+LBh9y+W4KQ=";
    };
  });

  unstable = import inputs.nixpkgs-unstable {
    inherit (prev.stdenv.hostPlatform) system;
    config.allowUnfree = true;
  };
}
