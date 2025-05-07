{
  pkgs,
  inputs,
  outputs,
  ...
}:
with inputs;
{
  imports = [
    ../modules/boot.nix
    ../modules/networking.nix

    home-manager.nixosModules.home-manager

    ../modules/users.nix
  ];

  system.stateVersion = "24.11";

  nixpkgs = {
    overlays = [
      outputs.overlays.additions
      outputs.overlays.modifications
      outputs.overlays.unstable-packages
    ];
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
    fwupd.enable = true;
    fstrim.enable = true;
    logrotate.checkConfig = false;
    keybase.enable = true;
  };

  home-manager = {
    useGlobalPkgs = true;
    useUserPackages = true;
    backupFileExtension = "home-manager-backup";
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
    rar
    ripgrep
    statix
    unrar
    unzip
    usbutils
    uv
    vim
    wget
    xh
    yamlfmt
    zip
  ];
}
