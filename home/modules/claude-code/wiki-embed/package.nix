{ lib, pkgs }:
let
  python = pkgs.python3.override {
    packageOverrides = self: super: {
      # torchWithRocm refers back to self.torch and would recurse in this override.
      torch = super.torch.override {
        triton = self.triton-no-cuda;
        rocmSupport = true;
        cudaSupport = false;
      };
    };
  };
  environment = python.withPackages (ps: [
    ps.sentence-transformers
    ps.transformers
    ps.torch
    ps.numpy
    ps.einops
  ]);
  testPython = pkgs.python3.withPackages (ps: [ ps.numpy ]);
in
pkgs.stdenvNoCC.mkDerivation {
  pname = "wiki-embed";
  version = "0.1.0";
  src = ./.;
  nativeBuildInputs = [ pkgs.makeWrapper ];
  dontBuild = true;
  doCheck = true;
  checkPhase = ''
    runHook preCheck
    export PYTHONDONTWRITEBYTECODE=1
    ${lib.getExe testPython} -m unittest discover -s tests -v
    runHook postCheck
  '';
  installPhase = ''
    runHook preInstall
    mkdir -p $out/lib/wiki-embed
    cp -r wiki_embed $out/lib/wiki-embed/
    makeWrapper ${lib.getExe environment} $out/bin/wiki-embed \
      --prefix PYTHONPATH : $out/lib/wiki-embed \
      --add-flags "-m wiki_embed"
    makeWrapper $out/bin/wiki-embed $out/bin/wiki-embed-client \
      --add-flags "--client"
    runHook postInstall
  '';
  meta.mainProgram = "wiki-embed";
}
