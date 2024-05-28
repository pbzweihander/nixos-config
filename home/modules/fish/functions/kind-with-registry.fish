function kind-with-registry
    set reg_name kind-registry
    set reg_port 5001
    if test "$(podman inspect -f '{{.State.Running}}' $reg_name 2>/dev/null)" != true
        podman run -d --restart=always -p "127.0.0.1:$reg_port:5000" --network bridge --name $reg_name registry:2
    end

    echo 'kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
containerdConfigPatches:
- |-
  [plugins."io.containerd.grpc.v1.cri".registry]
    config_path = "/etc/containerd/certs.d"
' | kind create cluster --config=- $argv

    set REGISTRY_DIR "/etc/containerd/certs.d/localhost:$reg_port"
    for node in (kind get nodes)
        podman exec $node mkdir -p $REGISTRY_DIR
        echo "[host.\"http://$reg_name:5000\"]
" | podman exec -i $node cp /dev/stdin "$REGISTRY_DIR/hosts.toml"
    end

    if test "$(podman inspect -f='{{json .NetworkSettings.Networks.kind}}' $reg_name)" = null
        podman network connect kind $reg_name
    end

    echo 'apiVersion: v1
kind: ConfigMap
metadata:
  name: local-registry-hosting
  namespace: kube-public
data:
  localRegistryHosting.v1: |
    host: "localhost:${reg_port}"
    help: "https://kind.sigs.k8s.io/docs/user/local-registry/"
' | kubectl apply -f -
end
