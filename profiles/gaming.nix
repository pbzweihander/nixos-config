{ pkgs, ... }:
let
  patchedBwrap = pkgs.bubblewrap.overrideAttrs (o: {
    patches = (o.patches or [ ]) ++ [
      ./bwrap.patch
    ];
  });
in
{
  programs = {
    steam = {
      enable = true;
      remotePlay.openFirewall = true;
      localNetworkGameTransfers.openFirewall = true;
      protontricks.enable = true;
      fontPackages = with pkgs; [ wqy_zenhei ];
      extraCompatPackages = with pkgs; [ proton-ge-bin ];
      # https://github.com/NixOS/nixpkgs/issues/217119
      package = pkgs.steam.override {
        buildFHSEnv = (
          args:
          (
            (pkgs.buildFHSEnv.override {
              bubblewrap = patchedBwrap;
            })
            (
              args
              // {
                extraBwrapArgs = (args.extraBwrapArgs or [ ]) ++ [ "--cap-add ALL" ];
              }
            )
          )
        );
      };
    };
    gamescope = {
      enable = true;
      capSysNice = true;
    };
  };

  environment.systemPackages = with pkgs; [
    gamescope
    unstable.nexusmods-app-unfree
    prismlauncher
    steamtinkerlaunch
    vesktop

    # https://github.com/NixOS/nixpkgs/issues/292620
    # https://github.com/NixOS/nixpkgs/issues/217119
    patchedBwrap
    (lutris.override {
      buildFHSEnv = (
        args:
        (
          (pkgs.buildFHSEnv.override {
            bubblewrap = patchedBwrap;
          })
          (
            args
            // {
              extraBwrapArgs = (args.extraBwrapArgs or [ ]) ++ [ "--cap-add ALL" ];
            }
          )
        )
      );
    })
  ];
}
