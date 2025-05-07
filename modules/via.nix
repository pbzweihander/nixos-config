{ pkgs, ... }:
{
  hardware.keyboard.qmk.enable = true;
  services.udev.packages = [ pkgs.via ];
}
