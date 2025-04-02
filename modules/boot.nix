{
  boot = {
    loader.systemd-boot.enable = true;
    loader.systemd-boot.editor = false;
    loader.efi.canTouchEfiVariables = true;
    loader.efi.efiSysMountPoint = "/efi";
  };
}
