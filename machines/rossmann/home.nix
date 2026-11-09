{ pkgs, inputs, ... }:
{
  imports = [
    ../../home/profiles/basic.nix
    ../../home/profiles/graphical.nix
    ../../home/profiles/gaming.nix

    ../../home/modules/claude-code/wiki-recap
    ../../home/modules/claude-code/wiki-embed
    ../../home/modules/claude-code/wiki-curator
    ../../home/modules/kde.nix
    ../../home/modules/ab9-telemetryd.nix
  ];

  home.file.".links/nix-citizen-wine-astral/bin".source = "${
    inputs.nix-citizen.packages.${pkgs.stdenv.hostPlatform.system}.wine-astral
  }/bin";

  # MOZA AB9 telemetry effects; see home/modules/ab9-telemetryd.nix before arming.
  services.ab9-telemetryd.armed = true;

  i18n.inputMethod.fcitx5.addons = with pkgs; [ fcitx5-mozc ];
}
