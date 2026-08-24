{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-26.05";
    multiverse.url = "github:fzakaria/nixpkgs-multiverse";

    systems.url = "github:nix-systems/default";
    srvos.url = "github:nix-community/srvos";
    nixos-hardware = {
      url = "github:NixOs/nixos-hardware";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    home-manager = {
      url = "github:nix-community/home-manager?ref=release-26.05";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    plasma-manager = {
      url = "github:nix-community/plasma-manager";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        home-manager.follows = "home-manager";
      };
    };

    nix-index-database = {
      url = "github:nix-community/nix-index-database";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nix-gaming.url = "github:fufexan/nix-gaming";

    nix-citizen = {
      url = "github:LovingMelody/nix-citizen";
      inputs.nix-gaming.follows = "nix-gaming";
    };

    nixos-apple-silicon = {
      url = "github:nix-community/nixos-apple-silicon";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nixpkgs-unstable.url = "github:nixos/nixpkgs?ref=nixos-unstable"; # remove when netbird module fixed
  };

  outputs =
    {
      nixpkgs,
      systems,
      ...
    }@inputs:
    let
      forAllSystems =
        function: nixpkgs.lib.genAttrs (import systems) (system: function nixpkgs.legacyPackages.${system});
    in
    {
      formatter = forAllSystems (pkgs: pkgs.nixfmt);

      nixosConfigurations = {
        linnamaa = nixpkgs.lib.nixosSystem {
          specialArgs = { inherit inputs; };
          modules = [
            "${inputs.nixpkgs-unstable}/nixos/modules/services/networking/netbird.nix"

            ./machines/linnamaa
          ];
        };

        rossmann = nixpkgs.lib.nixosSystem {
          specialArgs = { inherit inputs; };
          modules = [
            "${inputs.nixpkgs-unstable}/nixos/modules/services/networking/netbird.nix"

            ./machines/rossmann
          ];
        };

        juutilainen = nixpkgs.lib.nixosSystem {
          specialArgs = { inherit inputs; };
          modules = [
            "${inputs.nixpkgs-unstable}/nixos/modules/services/networking/netbird.nix"

            ./machines/juutilainen
          ];
        };

        schnaufer = nixpkgs.lib.nixosSystem {
          specialArgs = { inherit inputs; };
          modules = [
            "${inputs.nixpkgs-unstable}/nixos/modules/services/networking/netbird.nix"

            ./machines/schnaufer
          ];
        };
      };
    };
}
