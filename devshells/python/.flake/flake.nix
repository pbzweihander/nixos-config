{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=<nixos-ref>";
    # nixpkgs-python.url = "github:cachix/nixpkgs-python";
  };

  outputs =
    {
      nixpkgs,
      # nixpkgs-python,
      ...
    }:
    let
      overlays = [ ];
      forAllSystems =
        function:
        nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed (
          system:
          function (
            import nixpkgs {
              inherit system overlays;
            }
          )
        );
    in
    {
      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            python3
            poetry
            ruff
            uv
          ];
          shellHook = ''
            export LD_LIBRARY_PATH=${pkgs.stdenv.cc.cc.lib}/lib/
          '';
        };
      });
    };
}
