{
  nixConfig = {
    extra-substituters = ["https://nixpkgs-python.cachix.org"];
    extra-trusted-public-keys = [
      "nixpkgs-python.cachix.org-1:hxjI7pFxTyuTHn2NkvWCrAUcNZLNS3ZAvfYNuYifcEU="
    ];
  };

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-24.11";
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs-python.url = "github:cachix/nixpkgs-python";
  };

  outputs = {
    nixpkgs,
    flake-utils,
    nixpkgs-python,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      version = "<version>";
      overlays = [];
      pkgs = import nixpkgs {inherit system overlays;};
      python = nixpkgs-python.packages.${system}.${version};
    in {
      devShells.default = pkgs.mkShell {
        nativeBuildInputs = with pkgs; [python poetry ruff];
      };
    });
}
