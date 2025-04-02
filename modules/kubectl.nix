{pkgs, ...}: {
  environment.systemPackages = with pkgs; [
    kubectl
    kubectx
    kubernetes-helm
    kubectl-node-shell
  ];
}
