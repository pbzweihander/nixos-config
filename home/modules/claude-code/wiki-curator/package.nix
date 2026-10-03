{ lib, pkgs }:
pkgs.stdenvNoCC.mkDerivation {
  pname = "wiki-curator";
  version = "0.1.0";
  src = ./.;
  nativeBuildInputs = [ pkgs.makeWrapper ];
  nativeCheckInputs = [
    pkgs.python3
    pkgs.git
  ];
  dontBuild = true;
  doCheck = true;
  checkPhase = ''
    runHook preCheck
    export PYTHONDONTWRITEBYTECODE=1
    ${lib.getExe pkgs.python3} -m unittest discover -s tests -v
    runHook postCheck
  '';
  installPhase = ''
    runHook preInstall
    mkdir -p $out/lib/wiki-curator
    cp -r wiki_curator $out/lib/wiki-curator/
    makeWrapper ${lib.getExe pkgs.python3} $out/bin/wiki-curator \
      --prefix PYTHONPATH : $out/lib/wiki-curator \
      --add-flags "-m wiki_curator"
    runHook postInstall
  '';
  meta.mainProgram = "wiki-curator";
}
