{
  pkgs,
  inputs,
  ...
}:
with inputs; let
  hostname = "rossmann";
in {
  imports = [
    ./hardware-configuration.nix

    nixos-hardware.nixosModules.common-cpu-amd
    nixos-hardware.nixosModules.common-gpu-amd

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
    ../../profiles/kde.nix
    ../../profiles/dev.nix
    ../../profiles/gaming.nix
    ../../profiles/work.nix

    ../../modules/virt-manager.nix
    ../../modules/via.nix
  ];

  services = {
    btrfs.autoScrub.enable = true;
    tailscale.enable = true;
  };

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  networking = {
    hostName = hostname;
    firewall.allowedUDPPorts = [2934 2935 9987 9988 9989];
  };

  environment.systemPackages = with pkgs; [opentrack lact];

  systemd.services.lact = {
    description = "AMDGPU Control Daemon";
    after = ["multi-user.target"];
    wantedBy = ["multi-user.target"];
    serviceConfig = {ExecStart = "${pkgs.lact}/bin/lact daemon";};
    enable = true;
  };

  hardware.cpu.amd.updateMicrocode = true;

  boot.kernelPackages = pkgs.linuxPackages_latest;
}
