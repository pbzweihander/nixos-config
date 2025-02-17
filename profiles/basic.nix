{ pkgs, inputs, ... }:

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
    overlays = [ (import ../overlays) ];
    config.allowUnfree = true;
  };
  nix = {
    settings = {
      trusted-users = [ "root" "@wheel" ];
      extra-experimental-features = [ "nix-command" "flakes" ];
      extra-substituters = [ "https://nix-community.cachix.org" ];
      extra-trusted-public-keys = [
        "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
      ];
    };
    gc = {
      automatic = true;
      dates = "weekly";
      options = "--delete-older-than 7d";
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
    nixfmt-classic
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

