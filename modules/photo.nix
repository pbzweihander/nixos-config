{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [
    unstable.darktable
    unstable.rawtherapee
  ];
}
