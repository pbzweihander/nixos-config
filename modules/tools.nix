{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    lsd
    ripgrep
    fzf
    usbutils
    pciutils
    unzip
    mise
    pipx
    nil
  ];

  services.keybase.enable = true;
}
