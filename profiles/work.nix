{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [
    slack
    oci-cli
  ];
}
