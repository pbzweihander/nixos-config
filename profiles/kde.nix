{
  pkgs,
  inputs,
  ...
}:
with inputs; {
  services = {
    displayManager.sddm = {
      enable = true;
      wayland = {
        enable = true;
        compositor = "kwin";
      };
      theme = "arona";
    };
    desktopManager.plasma6.enable = true;
  };

  programs.kdeconnect.enable = true;

  environment = {
    plasma6.excludePackages = with pkgs.kdePackages; [konsole oxygen];
    systemPackages = with pkgs; [sddm-arona kdePackages.sddm-kcm];
  };

  home-manager.sharedModules = [plasma-manager.homeManagerModules.plasma-manager];

  # TODO: Replace with home-manager fcitx5 settings when 25.11
  i18n.inputMethod = {
    enable = true;
    type = "fcitx5";
    fcitx5 = {
      addons = with pkgs; [fcitx5-gtk fcitx5-hangul fcitx5-configtool];
      waylandFrontend = true;
    };
  };
}
