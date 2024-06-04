{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    nodejs_22
    nodePackages.typescript-language-server
    yarn
  ];
}
