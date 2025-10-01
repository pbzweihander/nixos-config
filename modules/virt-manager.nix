{ pkgs, ... }:
{
  programs.virt-manager.enable = true;
  users.groups = {
    kvm.members = [ "pbzweihander" ];
    libvirtd.members = [ "pbzweihander" ];
  };
  virtualisation = {
    libvirtd = {
      enable = true;
      qemu = {
        package = pkgs.qemu_kvm;
        runAsRoot = true;
        swtpm.enable = true;
        ovmf = {
          packages = [
            (pkgs.OVMF.override {
              secureBoot = true;
              tpmSupport = true;
            }).fd
          ];
        };
      };
    };
    spiceUSBRedirection.enable = true;
  };
}
