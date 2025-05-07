{
  pkgs,
  inputs,
  config,
  ...
}:
with inputs;
{
  boot.extraModulePackages = with config.boot.kernelPackages; [ evdi ];

  services.udev.packages = with pkgs; [ displaylink ];

  powerManagement.powerDownCommands = ''
    #flush any bytes in pipe
    while read -n 1 -t 1 SUSPEND_RESULT < /tmp/PmMessagesPort_out; do : ; done;

    #suspend DisplayLinkManager
    echo "S" > /tmp/PmMessagesPort_in

    #wait until suspend of DisplayLinkManager finish
    if [ -f /tmp/PmMessagesPort_out ]; then
      #wait until suspend of DisplayLinkManager finish
      read -n 1 -t 10 SUSPEND_RESULT < /tmp/PmMessagesPort_out
    fi
  '';

  powerManagement.resumeCommands = ''
    #resume DisplayLinkManager
    echo "R" > /tmp/PmMessagesPort_in
  '';

  systemd.services.dlm = {
    description = "DisplayLink Manager Service";
    after = [ "display-manager.service" ];
    conflicts = [ "getty@tty7.service" ];

    serviceConfig = {
      ExecStartPre = "${pkgs.kmod}/bin/modprobe evdi";
      ExecStart = "${pkgs.displaylink}/bin/DisplayLinkManager";
      Restart = "always";
      RestartSec = 5;
      LogsDirectory = "displaylink";
    };
  };

  environment.systemPackages = with pkgs; [ displaylink ];
}
