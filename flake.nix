{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";

    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-24.11";
    nixpkgs-unstable.url = "github:nixos/nixpkgs?ref=nixos-unstable";

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

    alejandra = {
      url = "github:kamadorueda/alejandra/3.1.0";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, alejandra, ... }@inputs: {
    formatter = alejandra.defaultPackage;
    nixosConfigurations = {
      linnamaa = nixpkgs.lib.nixosSystem {
        specialArgs = { inherit inputs; };
        modules = [ ./machines/linnamaa ];
      };

      rossmann = nixpkgs.lib.nixosSystem {
        specialArgs = { inherit inputs; };
        modules = [ ./machines/rossmann ];
      };

      juutilainen = nixpkgs.lib.nixosSystem {
        specialArgs = { inherit inputs; };
        modules = [ ./machines/juutilainen ];
      };
    };
  };
}
