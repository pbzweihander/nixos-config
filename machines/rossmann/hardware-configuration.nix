# Do not modify this file!  It was generated by ‘nixos-generate-config’
# and may be overwritten by future invocations.  Please make changes
# to /etc/nixos/configuration.nix instead.
{ config, lib, modulesPath, ... }:

{
  imports = [ (modulesPath + "/installer/scan/not-detected.nix") ];

  boot.initrd.availableKernelModules =
    [ "nvme" "xhci_pci" "ahci" "usbhid" "usb_storage" "sd_mod" ];
  boot.initrd.kernelModules = [ ];
  boot.kernelModules = [ "kvm-amd" ];
  boot.extraModulePackages = [ ];
  boot.resumeDevice = "/dev/disk/by-uuid/56af2d9f-6239-4953-a236-1040dae35d21";
  boot.kernelParams = [ "resume-offset=21964032" ];

  fileSystems."/" = {
    device = "/dev/disk/by-uuid/56af2d9f-6239-4953-a236-1040dae35d21";
    fsType = "btrfs";
    options = [ "subvol=system" ];
  };

  boot.initrd.luks.devices."cryptroot".device =
    "/dev/disk/by-uuid/b7cc38fb-34e8-4a00-862f-593f51123c6e";

  fileSystems."/efi" = {
    device = "/dev/disk/by-uuid/EB6E-E1B0";
    fsType = "vfat";
    options = [ "fmask=0022" "dmask=0022" ];
  };

  fileSystems."/home" = {
    device = "/dev/disk/by-uuid/56af2d9f-6239-4953-a236-1040dae35d21";
    fsType = "btrfs";
    options = [ "subvol=home" ];
  };

  fileSystems."/var/log" = {
    device = "/dev/disk/by-uuid/56af2d9f-6239-4953-a236-1040dae35d21";
    fsType = "btrfs";
    options = [ "subvol=log" ];
  };

  fileSystems."/swap" = {
    device = "/dev/disk/by-uuid/56af2d9f-6239-4953-a236-1040dae35d21";
    fsType = "btrfs";
    options = [ "subvol=@swap" ];
  };

  fileSystems."/home/pbzweihander/.local/share/Steam" = {
    device = "/dev/disk/by-uuid/56af2d9f-6239-4953-a236-1040dae35d21";
    fsType = "btrfs";
    options = [ "subvol=steam" ];
  };

  swapDevices = [{ device = "/swap/swapfile"; }];

  # Enables DHCP on each ethernet and wireless interface. In case of scripted networking
  # (the default) this is the recommended approach. When using systemd-networkd it's
  # still possible to use this option, but it's recommended to use it in conjunction
  # with explicit per-interface declarations with `networking.interfaces.<interface>.useDHCP`.
  networking.useDHCP = lib.mkDefault true;
  # networking.interfaces.enp5s0.useDHCP = lib.mkDefault true;

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.amd.updateMicrocode =
    lib.mkDefault config.hardware.enableRedistributableFirmware;
}
