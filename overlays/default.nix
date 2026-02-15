{ inputs }:
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

  spotify-qt = prev.spotify-qt.overrideAttrs (old: {
    version = "4.0.2-14d2ae6";
    src = final.fetchFromGitHub {
      owner = "kraxarn";
      repo = "spotify-qt";
      rev = "14d2ae6d41c7f3943827b930cf868499f7cff85d";
      hash = "sha256-V45LdbK4V5Lehfm8+eyeJdKKqx89gGbKCPyK+4xBBVA=";
    };
  });

  unstable = import inputs.nixpkgs-unstable {
    inherit (prev.stdenv.hostPlatform) system;
    config.allowUnfree = true;
  };
}
