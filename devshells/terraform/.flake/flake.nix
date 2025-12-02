{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=<nixos-ref>";
    flake-utils.url = "github:numtide/flake-utils";
    nixpkgs-terraform.url = "github:stackbuilders/nixpkgs-terraform";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      nixpkgs-terraform,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        version = "<version>";
        pkgs = import nixpkgs {
          inherit system;
        };
        terraform = nixpkgs-terraform.packages.${system}.${version};
      in
      {
        formatter = pkgs.nixfmt-rfc-style;
        devShells.default = pkgs.mkShell { nativeBuildInputs = [ terraform ]; };
      }
    );
}
