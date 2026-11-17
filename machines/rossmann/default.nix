{
  pkgs,
  inputs,
  ...
}:
with inputs;
let
  hostname = "rossmann";
  gameScopeArgs = [
    "--borderless"
    "--fullscreen"
    "-W"
    "3840"
    "-H"
    "2160"
    "-r"
    "160"
    "--hdr-enabled"
    "--adaptive-sync"
    "--prefer-output"
    "DP-1"
    "--mouse-sensitivity"
    "3.5"
    "--force-grab-cursor"
    "--cursor-scale-height"
    "1080"
    "--hdr-debug-force-output"
  ];
in
{
  imports = [
    ./hardware-configuration.nix

    nixos-hardware.nixosModules.common-cpu-amd
    nixos-hardware.nixosModules.common-gpu-amd

    ../../profiles/basic.nix
    ../../profiles/graphical.nix
    ../../profiles/dev.nix
    ../../profiles/gaming.nix
    ../../profiles/work.nix

    ../../modules/daw.nix
    ../../modules/kde.nix
    ../../modules/photo.nix
    ../../modules/scanner.nix
    ../../modules/sunshine.nix
    ../../modules/via.nix
  ];

  services = {
    btrfs.autoScrub = {
      enable = true;
      fileSystems = [ "/" ];
    };
    openssh = {
      enable = true;
      settings = {
        PasswordAuthentication = false;
      };
    };
    # Rudder pedals report only ABS_X/Y/Z without buttons, so udev's input_id
    # classifies them as accelerometers and games don't see them as joysticks.
    udev.extraHwdb = ''
      # Thrustmaster T.Flight Rudder Pedals
      id-input:modalias:input:b0003v044FpB679*
       ID_INPUT_ACCELEROMETER=0
       ID_INPUT_JOYSTICK=1

      # Thrustmaster T-Pendular-Rudder
      id-input:modalias:input:b0003v044FpB68F*
       ID_INPUT_ACCELEROMETER=0
       ID_INPUT_JOYSTICK=1
    '';
  };

  # MOZA AB9 FFB base (346e:1000) is not in hid-universal-pidff's ID table, so
  # it falls back to hid-generic, which drops buttons past KEY_MAX ("Invalid
  # code 768 type 1"). Register it as a dynamic ID; once registered, later
  # hot-plugs in the same boot bind to hid-universal-pidff on their own.
  systemd.services.moza-ab9-universal-pidff = {
    description = "Bind MOZA AB9 FFB base to hid-universal-pidff";
    wantedBy = [ "multi-user.target" ];
    after = [ "systemd-modules-load.service" ];
    path = [ pkgs.kmod ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
    };
    script = ''
      drv=/sys/bus/hid/drivers/hid-universal-pidff
      modprobe hid-universal-pidff
      # EEXIST when the ID is already registered in this boot.
      echo '0003 0000346E 00001000' > "$drv/new_id" || true

      # new_id only attaches unbound devices; move any AB9 that enumerated
      # before this unit ran off hid-generic.
      for dev in /sys/bus/hid/devices/0003:346E:1000.*; do
        [ -e "$dev" ] || continue
        name=$(basename "$dev")
        [ "$(basename "$(readlink -f "$dev/driver")")" = hid-universal-pidff ] && continue
        if [ -e "$dev/driver" ]; then
          echo "$name" > "$dev/driver/unbind"
        fi
        echo "$name" > "$drv/bind"
      done
    '';
  };

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  networking = {
    hostName = hostname;
    firewall = {
      allowedUDPPorts = [
        # DCS-BIOS
        5010
      ];
      allowedUDPPortRanges = [
        {
          # Steam Game client
          from = 27005;
          to = 27032;
        }
      ];
      allowedTCPPorts = [
        # orca
        6768
      ];
    };
  };

  environment.systemPackages = with pkgs; [
    (nix-citizen.packages.${stdenv.hostPlatform.system}.star-citizen.override (prev: {
      inherit gameScopeArgs;
      preCommands = ''
        export DXVK_HDR=1
        env DISPLAY=:0 opentrack -platform xcb &
      '';
      gameScopeEnable = true;
    }))

    displaylink
    gamemode
    heroic
    lact
    opentrack
    p7zip
    quickemu
  ];

  nix.settings = {
    extra-substituters = [
      "https://nix-gaming.cachix.org"
      "https://nix-citizen.cachix.org"
      "https://ezkea.cachix.org"
    ];
    extra-trusted-public-keys = [
      "nix-gaming.cachix.org-1:nbjlureqMbRAxR1gJ/f3hxemL9svXaZF/Ees8vCUUs4="
      "nix-citizen.cachix.org-1:lPMkWc2X8XD4/7YPEEwXKKBg+SVbYTVrAaLA2wQTKCo="
      "ezkea.cachix.org-1:ioBmUbJTZIKsHmWWXPe1FSFbeVe+afhfgqgTSNd34eI="
    ];
  };

  systemd.services = {
    lact = {
      description = "AMDGPU Control Daemon";
      after = [ "multi-user.target" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        ExecStart = "${pkgs.lact}/bin/lact daemon";
      };
      enable = true;
    };
    displaylink-server = {
      enable = true;
      requires = [ "systemd-udevd.service" ];
      after = [ "systemd-udevd.service" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "simple";
        ExecStart = "${pkgs.displaylink}/bin/DisplayLinkManager";
        User = "root";
        Group = "root";
        Restart = "on-failure";
        RestartSec = 5;
      };
    };
  };

  hardware = {
    cpu.amd.updateMicrocode = true;
    amdgpu.opencl.enable = true;
  };

  boot = {
    # kernelPackages = pkgs.linuxPackages_latest;
    kernel.sysctl = {
      "vm.max_map_count" = 16777216;
      "fs.file-max" = 524288;
    };
  };

  virtualisation.spiceUSBRedirection.enable = true;

  # Use steam launch option `gamescope -- env DXVK_HDR=1 %command%` for HDR
  programs.gamescope.args = gameScopeArgs;

  users.users."pbzweihander".extraGroups = [
    "input"
  ];
}
