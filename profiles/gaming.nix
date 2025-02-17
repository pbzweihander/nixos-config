{ pkgs, ... }:

{
  programs = {
    steam = {
      enable = true;
      remotePlay.openFirewall = true;
      localNetworkGameTransfers.openFirewall = true;
      protontricks.enable = true;
      fontPackages = with pkgs; [ wqy_zenhei ];
      extraCompatPackages = with pkgs; [ proton-ge-bin ];
    };
    gamescope = {
      enable = true;
      capSysNice = true;
    };
  };

  environment.systemPackages = with pkgs; [
    gamemode
    gamescope
    steamtinkerlaunch
    vesktop
  ];
}
