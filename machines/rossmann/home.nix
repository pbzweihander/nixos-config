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
  services.ab9-telemetryd = {
    armed = true;
    # Scales every telemetry effect (0.25..4.0); per-aircraft tweaks go to
    # ~/.config/ab9-ffb/telemetry-overrides.json.
    gain = 0.5;
  };

  i18n.inputMethod.fcitx5.addons = with pkgs; [ fcitx5-mozc ];
}
