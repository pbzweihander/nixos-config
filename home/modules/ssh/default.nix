{
  programs.ssh = {
    enable = true;
    matchBlocks = {
      github = {
        hostname = "github.com";
        user = "git";
      };
      gitlab = {
        hostname = "gitlab.com";
        user = "git";
      };
      rossmann = {
        hostname = "192.168.8.247"; # via tailscale
        user = "pbzweihander";
      };
    };
  };
}
