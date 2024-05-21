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
  };

  home-manager = {
    useGlobalPkgs = true;
    useUserPackages = true;
    sharedModules = [ plasma-manager.homeManagerModules.plasma-manager ];
  };

  services.logrotate.checkConfig = false;

  environment.systemPackages = with pkgs; [
    helix
    vim
    lsd
    ripgrep
    fzf
    usbutils
    pciutils
    unzip
    mise
    pipx
    nil
    dogdns
  ];

  services.keybase.enable = true;
}

