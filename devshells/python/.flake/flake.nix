{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=<nixos-ref>";
    flake-utils.url = "github:numtide/flake-utils";
    # nixpkgs-python.url = "github:cachix/nixpkgs-python";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      # nixpkgs-python,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        # version = "<version>";
        overlays = [ ];
        pkgs = import nixpkgs { inherit system overlays; };
        # python3 = nixpkgs-python.packages.${system}.${version};
      in
      {
        formatter = pkgs.nixfmt-rfc-style;
        devShells.default = pkgs.mkShell {
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
      }
    );
}
