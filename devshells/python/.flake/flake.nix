{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs-python.url = "github:cachix/nixpkgs-python";
  };

  outputs = { nixpkgs, flake-utils, nixpkgs-python, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        version = "<version>";
        overlays = [ ];
        pkgs = import nixpkgs { inherit system overlays; };
        python = nixpkgs-python.packages.${system}.${version};
      in {
        devShells.default =
          pkgs.mkShell { nativeBuildInputs = with pkgs; [ python poetry ]; };
      });
}
