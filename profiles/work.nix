{ pkgs, config, ... }:
{
  environment.systemPackages = with pkgs; [
    (
      if stdenv.hostPlatform.system == "aarch64-linux" then
        config.multiverse.instance.fast.tip.slacky
      else
        slack
    )
    oci-cli
  ];
  services.netbird = {
    enable = true;
    package = config.multiverse.pinned.netbird;
    ui = {
      package = config.multiverse.pinned.netbird-ui;
      enable = true;
    };
    clients.default = {
      openFirewall = true;
      openInternalFirewall = true;
    };
  };
}
