{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=<nixos-ref>";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      overlays = [ (import rust-overlay) ];
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
      devShells = forAllSystems (
        pkgs:
        let
          version = "<version>";
          rust-toolchain = pkgs.rust-bin.stable.${version}.default;
        in
        {
          default = pkgs.mkShell { nativeBuildInputs = [ rust-toolchain ]; };
        }
      );
    };
}
