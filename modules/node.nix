{pkgs, ...}: {
  environment.systemPackages = with pkgs; [
    nodejs
    nodePackages.typescript-language-server
    yarn
  ];
}
