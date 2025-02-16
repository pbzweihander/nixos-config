{ pkgs, inputs, ... }:

with inputs;

{
  services = {
    displayManager.sddm = {
      enable = true;
      enableHidpi = true;
    };
    desktopManager.plasma6.enable = true;
  };

  programs.kdeconnect.enable = true;

  environment = {
    plasma6.excludePackages = with pkgs.kdePackages; [ konsole oxygen ];
    systemPackages = with pkgs; [ sddm-arona ];
  };

  home-manager.sharedModules =
    [ plasma-manager.homeManagerModules.plasma-manager ];
}
