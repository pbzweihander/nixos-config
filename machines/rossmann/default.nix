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

    ../../modules/kde.nix
    ../../modules/via.nix
    ../../modules/daw.nix
    ../../modules/photo.nix

    aagl.nixosModules.default
  ];

  services = {
    btrfs.autoScrub = {
      enable = true;
      fileSystems = [ "/" ];
    };
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
    (nix-citizen.packages.${stdenv.hostPlatform.system}.star-citizen.override (prev: {
      inherit gameScopeArgs;
      preCommands = ''
        export DXVK_HDR=1
        env DISPLAY=:0 opentrack -platform xcb &
      '';
      gameScopeEnable = true;
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

  systemd.services.lact = {
    description = "AMDGPU Control Daemon";
    after = [ "multi-user.target" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      ExecStart = "${pkgs.lact}/bin/lact daemon";
    };
    enable = true;
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

  programs = {
    honkers-railway-launcher.enable = true;

    # Use steam launch option `gamescope -- env DXVK_HDR=1 %command%` for HDR
    gamescope.args = gameScopeArgs;
  };
}
