{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=<nixos-ref>";
  };

  outputs =
    {
      nixpkgs,
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
        default = pkgs.mkShell { nativeBuildInputs = with pkgs; [ ]; };
      });
    };
}
