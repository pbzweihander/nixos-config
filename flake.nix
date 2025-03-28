{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";

    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-24.11";
    nixos-hardware.url = "github:NixOs/nixos-hardware";

    home-manager = {
      url = "github:nix-community/home-manager?ref=release-24.11";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    plasma-manager = {
      url = "github:pjones/plasma-manager";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.home-manager.follows = "home-manager";
    };
  };

  outputs = { self, flake-utils, nixpkgs, ... }@inputs:
    let systems = [ "x86_64-linux" ];
    in flake-utils.lib.eachSystem systems (system:
      let pkgs = nixpkgs.legacyPackages.${system};
      in { formatter = pkgs.nixfmt-classic; }) // {
        nixosConfigurations = {
          linnamaa = nixpkgs.lib.nixosSystem {
            system = "x86_64-linux";
            specialArgs = { inherit inputs; };
            modules = [ ./machines/linnamaa ];
          };

          rossmann = nixpkgs.lib.nixosSystem {
            system = "x86_64-linux";
            specialArgs = { inherit inputs; };
            modules = [ ./machines/rossmann ];
          };

          juutilainen = nixpkgs.lib.nixosSystem {
            system = "x86_64-linux";
            specialArgs = { inherit inputs; };
            modules = [ ./machines/juutilainen ];
          };
        };
      };
}
