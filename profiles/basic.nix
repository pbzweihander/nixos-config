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

  nixpkgs.config.allowUnfree = true;
  nix.settings = {
    trusted-users = [ "root" "@wheel" ];
    extra-experimental-features = [ "nix-command" "flakes" ];
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
    sharedModules = [ plasma-manager.homeManagerModules.plasma-manager ];
  };

  environment.systemPackages = with pkgs; [
    bitwarden-cli
    dogdns
    fd
    fzf
    helix
    jaq
    lsd
    nil
    nixfmt-classic
    pciutils
    python3
    ripgrep
    unzip
    usbutils
    vim
  ];
}

