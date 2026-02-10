{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [
    (if stdenv.hostPlatform.system == "aarch64-linux" then unstable.slacky else slack)
    oci-cli
  ];
}
