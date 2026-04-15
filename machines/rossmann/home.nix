{ pkgs, inputs, ... }:
{
  imports = [
    ../../home/profiles/basic.nix
    ../../home/profiles/graphical.nix
    ../../home/profiles/gaming.nix

    ../../home/modules/kde.nix
  ];

  home.file.".links/nix-citizen-wine-astral/bin".source = "${
    inputs.nix-citizen.packages.${pkgs.stdenv.hostPlatform.system}.wine-astral
  }/bin";

  i18n.inputMethod.fcitx5.addons = with pkgs; [ fcitx5-mozc ];
}
