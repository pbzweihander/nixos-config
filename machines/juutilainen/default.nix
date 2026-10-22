{
  inputs,
  pkgs,
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
    ../../profiles/dev.nix
    ../../profiles/work.nix

    ../../modules/kde.nix
    ../../modules/photo.nix
    ../../modules/scanner.nix
    ../../modules/via.nix
    ../../modules/wifi.nix
  ];

  services = {
    btrfs.autoScrub = {
      enable = true;
      fileSystems = [ "/" ];
    };
    fstrim.enable = true;
    xserver.xkb.options = "ctrl:nocaps,korean:ralt_hangul,korean:rctrl_hanja";
  };

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  networking.hostName = hostname;

  hardware = {
    bluetooth.enable = true;
    graphics.extraPackages = with pkgs; [
      intel-compute-runtime
    ];
  };

  boot.kernel.sysctl = {
    "vm.max_map_count" = 16777216;
    "fs.file-max" = 524288;
  };

  # Under sustained load the EC cuts the MMIO PL1 to 12 W once its SEN2 sensor
  # reaches ~59 C, stalling the CPU at 400 MHz and starving the iGPU.
  # Capping the MSR PL1 at 20 W keeps SEN2 below the cut (measured over 9 min).
  # Do not enable thermald: on this DYTC platform it pins PL1 at 10 W.
  # The sleep targets are reached only after resume, so ordering after them
  # reapplies the limits on wake.
  systemd.services.rapl-power-limits =
    let
      sleepTargets = [
        "suspend.target"
        "hibernate.target"
        "hybrid-sleep.target"
        "suspend-then-hibernate.target"
      ];
    in
    {
      description = "Cap package power so the EC does not cut PL1 to 12 W";
      wantedBy = [ "multi-user.target" ] ++ sleepTargets;
      after = sleepTargets;
      serviceConfig.Type = "oneshot";
      script = ''
        d=/sys/class/powercap/intel-rapl:0
        echo 27983872 > $d/constraint_0_time_window_us
        echo 20000000 > $d/constraint_0_power_limit_uw
        echo 35000000 > $d/constraint_1_power_limit_uw
      '';
    };
}
