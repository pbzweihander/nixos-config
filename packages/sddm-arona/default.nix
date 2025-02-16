{ pkgs, stdenvNoCC, ... }:

stdenvNoCC.mkDerivation {
  pname = "sddm-arona";
  version = "1.0";

  src = pkgs.fetchFromGitHub {
    owner = "Machillka";
    repo = "Arona-sddm-login";
    rev = "abbe71cfacf86b688cf04e54e2820195fd956631";
    hash = "sha256-5EaGP+2f/jX0hpbj8qKiEGLRBxQO+c2/Gu/brcVeTTM=";
  };

  patches = [ ./0-qt5-compat.patch ];

  dontBuild = true;

  installPhase = let basePath = "$out/share/sddm/themes";
  in ''
    mkdir -p ${basePath}
    cp -aR . ${basePath}/arona
  '';
}
