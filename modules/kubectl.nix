{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    kubectl
    kubectx
    kubernetes-helm
    kubernetes-helmPlugins.helm-diff
    kubernetes-helmPlugins.helm-s3
  ];
}
