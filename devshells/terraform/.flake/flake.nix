{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs-terraform.url = "github:stackbuilders/nixpkgs-terraform";
  };

  outputs = { nixpkgs, flake-utils, nixpkgs-terraform, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        version = "<version>";
        overlays = [ nixpkgs-terraform.overlays.default ];
        pkgs = import nixpkgs { inherit system overlays; };
        terraform = pkgs.terraform-versions.${version};
      in {
        devShells.default = pkgs.mkShell { nativeBuildInputs = [ terraform ]; };
      });
}
