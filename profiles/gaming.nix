{ pkgs, config, ... }:
{
  programs = {
    steam = {
      enable = true;
      remotePlay.openFirewall = true;
      localNetworkGameTransfers.openFirewall = true;
      protontricks.enable = true;
      fontPackages = with pkgs; [ wqy_zenhei ];
      extraCompatPackages = with pkgs; [ proton-ge-bin ];
    };
    gamescope.enable = true;
  };

  services.ananicy = {
    enable = true;
    package = pkgs.ananicy-cpp;
    rulesProvider = pkgs.ananicy-cpp;
    extraRules = [
      {
        "name" = "gamescope";
        "nice" = -20;
      }
    ];
  };

  environment.systemPackages =
    (with pkgs; [
      gamescope-wsi
      limo
      lutris
      prismlauncher
      steamtinkerlaunch
      vesktop
      xivlauncher
    ])
    ++ (with config.multiverse.instance.fast.tip; [
      protonplus
    ]);
}
