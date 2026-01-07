{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=<nixos-ref>";
    nixpkgs-terraform.url = "github:stackbuilders/nixpkgs-terraform";
  };

  outputs =
    {
      nixpkgs,
      nixpkgs-terraform,
      ...
    }:
    let
      overlays = [ nixpkgs-terraform.overlays.default ];
      forAllSystems =
        function:
        nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed (
          system:
          function (
            import nixpkgs {
              inherit system overlays;
              config.allowUnfree = true;
            }
          )
        );
    in
    forAllSystems (
      pkgs:
      let
        version = "<version>";
        terraform = pkgs.terraform-versions.${version};
      in
      {
        formatter = pkgs.nixfmt-rfc-style;
        devShells.default = pkgs.mkShell { nativeBuildInputs = [ terraform ]; };
      }
    );
}
