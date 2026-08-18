{ pkgs, inputs, ... }:
let
  mv = inputs.multiverse.multiverse.${pkgs.stdenv.hostPlatform.system};
in
{
  imports = [
    ../modules/sound.nix
    ../modules/fonts.nix
  ];

  environment = {
    systemPackages =
      with pkgs;
      [
        ghostty
        gimp
        inkscape
        kdePackages.kcalc
        kdePackages.kolourpaint
        vlc
        vscode
        wl-clipboard

        bubblewrap # needed by zed
      ]
      ++ (with mv.tip; [ zed-editor ])
      ++ (if stdenv.hostPlatform.system == "aarch64-linux" then [ ] else [ spotify ]);
    sessionVariables.NIXOS_OZONE_WL = "1";
  };

  services = {
    xserver = {
      enable = false;
      excludePackages = with pkgs; [ xterm ];
    };
  };

  programs.firefox.enable = true;
}
