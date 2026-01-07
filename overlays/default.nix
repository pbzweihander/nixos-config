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

  unstable = import inputs.nixpkgs-unstable {
    system = prev.stdenv.hostPlatform.system;
    config.allowUnfree = true;
  };
}
