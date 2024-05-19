{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    ripgrep
    fzf
    usbutils
    pciutils
    unzip
    mise
  ];

  services.keybase.enable = true;
}
