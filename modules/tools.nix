{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    ripgrep
    fzf
    usbutils
    pciutils
    mise
  ];
}
