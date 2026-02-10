{
  lib,
  modulesPath,
  ...
}:

{
  imports = [
    (modulesPath + "/installer/scan/not-detected.nix")
  ];

  boot.initrd.availableKernelModules = [
    "usb_storage"
    "sdhci_pci"
  ];
  boot.initrd.kernelModules = [ ];
  boot.kernelModules = [ ];
  boot.extraModulePackages = [ ];
  boot.loader.efi.canTouchEfiVariables = false;

  fileSystems."/" = {
    device = "/dev/mapper/cryptroot";
    fsType = "btrfs";
    options = [
      "subvol=system"
      "noatime"
    ];
  };

  boot.initrd.luks.devices."cryptroot" = {
    device = "/dev/disk/by-uuid/19c30518-6ed4-495f-bbd8-c61cfad97f15";
    bypassWorkqueues = true;
    allowDiscards = true;
  };

  fileSystems."/var/log" = {
    device = "/dev/mapper/cryptroot";
    fsType = "btrfs";
    options = [
      "subvol=log"
      "noatime"
    ];
  };

  fileSystems."/nix" = {
    device = "/dev/mapper/cryptroot";
    fsType = "btrfs";
    options = [
      "subvol=nix"
      "noatime"
    ];
  };

  fileSystems."/swap" = {
    device = "/dev/mapper/cryptroot";
    fsType = "btrfs";
    options = [
      "subvol=swap"
      "noatime"
    ];
  };

  fileSystems."/home" = {
    device = "/dev/mapper/cryptroot";
    fsType = "btrfs";
    options = [
      "subvol=home"
      "noatime"
    ];
  };

  fileSystems."/efi" = {
    device = "/dev/disk/by-uuid/81A5-19FD";
    fsType = "vfat";
    options = [
      "fmask=0022"
      "dmask=0022"
    ];
  };

  swapDevices = [ { device = "/swap/swapfile"; } ];

  nixpkgs.hostPlatform = lib.mkDefault "aarch64-linux";
}
