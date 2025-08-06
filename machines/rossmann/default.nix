{
  pkgs,
  inputs,
  ...
}:
with inputs;
let
  hostname = "rossmann";
in
{
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

    ../../modules/via.nix
  ];

  services = {
    btrfs.autoScrub.enable = true;
    input-remapper = {
      enable = true;
      enableUdevRules = true;
    };
    tailscale.enable = true;
    openssh = {
      enable = true;
      settings = {
        PasswordAuthentication = false;
      };
    };
  };

  time.timeZone = "Asia/Seoul";

  home-manager.users.pbzweihander = import ./home.nix;

  networking = {
    hostName = hostname;
    firewall = {
      allowedUDPPortRanges = [
        {
          # Steam Game client
          from = 27005;
          to = 27032;
        }
      ];
    };
  };

  environment.systemPackages = with pkgs; [
    (nix-citizen.packages.${system}.star-citizen.override (prev: {
      gameScopeEnable = true;
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
        "--force-grab-cursor"
        "--mouse-sensitivity"
        "2"
      ];
    }))
    (
      (mumble.overrideAttrs (prev: {
        postFixup = prev.postFixup + ''
          wrapProgram $out/bin/mumble \
            --set XDG_SESSION_TYPE x11
        '';
      })).override
      {
        speechdSupport = true;
      }
    )
    gamemode
    lact
    opentrack
  ];

  nix.settings = {
    substituters = [
      "https://nix-gaming.cachix.org"
      "https://nix-citizen.cachix.org"
    ];
    trusted-public-keys = [
      "nix-gaming.cachix.org-1:nbjlureqMbRAxR1gJ/f3hxemL9svXaZF/Ees8vCUUs4="
      "nix-citizen.cachix.org-1:lPMkWc2X8XD4/7YPEEwXKKBg+SVbYTVrAaLA2wQTKCo="
    ];
  };

  systemd.services.lact = {
    description = "AMDGPU Control Daemon";
    after = [ "multi-user.target" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      ExecStart = "${pkgs.lact}/bin/lact daemon";
    };
    enable = true;
  };

  hardware.cpu.amd.updateMicrocode = true;

  boot = {
    # kernelPackages = pkgs.linuxPackages_latest;
    kernel.sysctl = {
      "vm.max_map_count" = 16777216;
      "fs.file-max" = 524288;
    };
  };
}
