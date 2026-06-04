{
  programs.ssh = {
    enable = true;
    enableDefaultConfig = false;
    matchBlocks = {
      "*" = {
        forwardAgent = false;
        addKeysToAgent = "no";
        compression = false;
        serverAliveInterval = 0;
        serverAliveCountMax = 3;
        hashKnownHosts = false;
        userKnownHostsFile = "~/.ssh/known_hosts";
        controlMaster = "no";
        controlPath = "~/.ssh/master-%r@%n:%p";
        controlPersist = "no";
      };
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
