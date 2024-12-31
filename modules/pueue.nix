{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [ pueue ];

  # https://github.com/Nukesor/pueue/blob/8d376804a27bac3e22450085913e280ec57b4dad/utils/pueued.service
  systemd.user.services.pueued = {
    enable = true;
    description = "Pueue Daemon - CLI process scheduler and manager";
    wantedBy = [ "default.target" ];
    serviceConfig = {
      Restart = "no";
      ExecStart = "${pkgs.pueue}/bin/pueued -vv";
    };
  };
}
