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
    version = "4.0.4";
    src = final.fetchFromGitHub {
      owner = "kraxarn";
      repo = "spotify-qt";
      rev = "a5eeff4fa0459d507d3f7b8583cf6f5a53c43540";
      hash = "sha256-5stDBpt+TJGihrmEVt0kB4Aiez7wIpqm4P6RnSDquxM=";
    };
  });

  unstable = import inputs.nixpkgs-unstable {
    inherit (prev.stdenv.hostPlatform) system;
    config.allowUnfree = true;
  };
}
