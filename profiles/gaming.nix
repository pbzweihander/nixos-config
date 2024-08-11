{ pkgs, ... }:

{
  programs.steam = {
    enable = true;
    remotePlay.openFirewall = true;
    localNetworkGameTransfers.openFirewall = true;
    gamescopeSession.enable = true;
    protontricks.enable = true;
    fontPackages = with pkgs; [ wqy_zenhei ];
    extraCompatPackages = with pkgs; [ proton-ge-bin ];
  };
  programs.gamescope.enable = true;

  environment.systemPackages = with pkgs; [
    gamescope
    steamtinkerlaunch
    vesktop
  ];
}
