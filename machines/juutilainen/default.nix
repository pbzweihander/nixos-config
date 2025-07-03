{
  lib,
  inputs,
  outputs,
  ...
}:
with inputs;
let
  hostname = "juutilainen";
in
{
  imports = [
    ./hardware-configuration.nix

    nixos-hardware.nixosModules.lenovo-thinkpad-x1-12th-gen

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
    ../../profiles/kde.nix
    ../../profiles/dev.nix
    ../../profiles/work.nix

    ../../modules/via.nix
  ];

  services = {
    btrfs.autoScrub.enable = true;
    fstrim.enable = true;
    fprintd.enable = true;
    xserver.xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
    tailscale.enable = true;
  };

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  networking.hostName = hostname;

  hardware.bluetooth.enable = true;

  systemd.services.fprintd = {
    wantedBy = [ "multi-user.target" ];
    serviceConfig.Type = "simple";
  };

  security.pam.services.sddm.text = lib.mkForce (
    lib.strings.concatLines (
      builtins.filter (x: (lib.strings.hasPrefix "auth " x) && (!lib.strings.hasInfix "fprintd" x)) (
        lib.strings.splitString "\n"
          outputs.nixosConfigurations.${hostname}.config.security.pam.services.login.text
      )
    )
    + ''

      account   include   login
      password  substack  login
      session   include   login
    ''
  );
}
