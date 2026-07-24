{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [
    (if stdenv.hostPlatform.system == "aarch64-linux" then unstable.slacky else slack)
    oci-cli
  ];
  services.netbird = {
    enable = true;
    package = pkgs.netbird-update.netbird;
    ui = {
      enable = true;
      package = pkgs.netbird-update.netbird-ui;
    };
    clients.default = {
      openFirewall = true;
      openInternalFirewall = true;
    };
  };
}
