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
    nil
  ];

  services.keybase.enable = true;
}
