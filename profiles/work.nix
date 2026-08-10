{ pkgs, inputs, ... }:
let
  mv = inputs.multiverse.multiverse.${pkgs.stdenv.hostPlatform.system};
in
{
  environment.systemPackages = with pkgs; [
    (if stdenv.hostPlatform.system == "aarch64-linux" then mv.tip.slacky else slack)
    oci-cli
  ];
  services.netbird = {
    enable = true;
    # package = mv.tip.netbird;
    ui = {
      enable = true;
      # package = mv.tip.netbird-ui;
    };
    clients.default = {
      openFirewall = true;
      openInternalFirewall = true;
    };
  };
}
