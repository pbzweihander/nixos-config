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
      # Wine only uses its SDL joystick backend when libSDL2 is loadable. Without
      # it Wine falls back to raw evdev, whose button order (BTN_MODE before the
      # stick clicks) shifts LS/RS in XInput, and the Steam Controller is not
      # seen at all. SDL2 here is sdl2-compat, so SDL3's HIDAPI drivers apply.
      (xivlauncher.overrideAttrs (old: {
        dotnetRuntimeDeps = old.dotnetRuntimeDeps ++ [ SDL2 ];
      }))
    ])
    ++ (with config.multiverse.instance.fast.tip; [
      protonplus
    ]);
}
