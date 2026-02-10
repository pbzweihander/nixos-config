{
  pkgs,
  inputs,
  ...
}:
with inputs;
{
  imports = [
    ../modules/boot.nix
    ../modules/networking.nix

    home-manager.nixosModules.home-manager
    nix-index-database.nixosModules.nix-index

    ../modules/users.nix
  ];

  system.stateVersion = "25.11";

  nixpkgs = {
    overlays = [ (import ../overlays { inherit inputs; }) ];
    config.allowUnfree = true;
  };
  nix = {
    settings = {
      trusted-users = [
        "root"
        "@wheel"
      ];
      extra-experimental-features = [
        "nix-command"
        "flakes"
      ];
      extra-substituters = [ "https://nix-community.cachix.org" ];
      extra-trusted-public-keys = [
        "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
      ];
    };
  };

  services = {
    fstrim.enable = true;
    fwupd.enable = true;
    logrotate.checkConfig = false;
    lorri.enable = true;
  };

  home-manager = {
    useGlobalPkgs = true;
    useUserPackages = true;
    backupFileExtension = "home-manager-backup";
    extraSpecialArgs = {
      inherit inputs;
    };
  };

  programs = {
    nh = {
      enable = true;
      flake = "/home/pbzweihander/nixos-config";
      clean = {
        enable = true;
        extraArgs = "--keep-since 7d --keep 3";
      };
    };
    command-not-found.enable = false;
    nix-index-database.comma.enable = true;
    nix-index.enableFishIntegration = false;
  };

  environment.systemPackages = with pkgs; [
    direnv
    dogdns
    eva
    fd
    file
    fzf
    helix
    jaq
    lsd
    nil
    nixfmt-rfc-style
    nix-tree
    pciutils
    pwgen
    python3
    ripgrep
    statix
    unzip
    usbutils
    uv
    vim
    wget
    xh
    zip
  ];
}
