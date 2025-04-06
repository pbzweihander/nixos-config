{
  pkgs,
  inputs,
  ...
}:
with inputs; {
  imports = [
    ../modules/boot.nix
    ../modules/networking.nix

    home-manager.nixosModules.home-manager

    ../modules/users.nix
  ];

  system.stateVersion = "24.11";

  nixpkgs = {
    overlays = [
      (final: prev: {
        unstable = nixpkgs-unstable.legacyPackages.${prev.system};
      })
      (import ../overlays)
    ];
    config.allowUnfree = true;
  };
  nix = {
    settings = {
      trusted-users = ["root" "@wheel"];
      extra-experimental-features = ["nix-command" "flakes"];
      extra-substituters = ["https://nix-community.cachix.org"];
      extra-trusted-public-keys = [
        "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
      ];
    };
  };

  services = {
    fwupd.enable = true;
    fstrim.enable = true;
    logrotate.checkConfig = false;
    keybase.enable = true;
  };

  home-manager = {
    useGlobalPkgs = true;
    useUserPackages = true;
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
  };

  environment.systemPackages = with pkgs; [
    alejandra
    dogdns
    eva
    fd
    file
    fzf
    helix
    jaq
    lsd
    nil
    nix-tree
    pciutils
    pwgen
    python3
    rar
    ripgrep
    unrar
    unzip
    usbutils
    vim
    wget
    xh
    zip
  ];
}
