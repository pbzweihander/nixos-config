{ pkgs, inputs, ... }:

with inputs;

{
  imports = [
    ../modules/boot.nix
    ../modules/networking.nix

    home-manager.nixosModules.home-manager

    ../modules/users.nix
  ];

  system.stateVersion = "23.11";

  nixpkgs = {
    overlays = [ (import ../overlays) ];
    config.allowUnfree = true;
  };
  nix = {
    settings = {
      trusted-users = [ "root" "@wheel" ];
      extra-experimental-features = [ "nix-command" "flakes" ];
      trusted-substituters =
        [ "https://cache.nixos.org/" "https://nixpkgs-python.cachix.org" ];
      trusted-public-keys = [
        "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
        "nixpkgs-python.cachix.org-1:hxjI7pFxTyuTHn2NkvWCrAUcNZLNS3ZAvfYNuYifcEU="
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

