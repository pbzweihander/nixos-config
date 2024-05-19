{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    ripgrep
    fzf
    usbutils
    pciutils
    unzip
    mise
    pipx
  ];

  services.keybase.enable = true;
}
