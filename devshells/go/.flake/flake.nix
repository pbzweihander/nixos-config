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
    forAllSystems (pkgs: {
      formatter = pkgs.nixfmt-rfc-style;
      devShells.default = pkgs.mkShell {
        nativeBuildInputs = with pkgs; [
          go
          gopls
          golangci-lint
          golangci-lint-langserver
        ];
      };
    });
}
