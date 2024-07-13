{ pkgs, ... }:

{
  programs.steam = {
    enable = true;
    remotePlay.openFirewall = true;
    localNetworkGameTransfers.openFirewall = true;
    gamescopeSession.enable = true;
    fontPackages = with pkgs; [ wqy_zenhei ];
  };
  programs.gamescope.enable = true;

  environment.systemPackages = with pkgs; [ vesktop protontricks gamescope ];
}
