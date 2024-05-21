{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [ slack _1password _1password-gui ];
}
