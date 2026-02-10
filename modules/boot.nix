{ lib, ... }:
{
  boot = {
    loader = {
      systemd-boot = {
        enable = true;
        editor = false;
      };
      efi = {
        canTouchEfiVariables = lib.mkDefault true;
        efiSysMountPoint = "/efi";
      };
    };
  };
}
