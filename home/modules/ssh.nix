{
  programs.ssh = {
    enable = true;
    enableDefaultConfig = false;
    settings = {
      "*" = {
        ForwardAgent = false;
        AddKeysToAgent = "no";
        Compression = false;
        ServerAliveInterval = 0;
        ServerAliveCountMax = 3;
        HashKnownHosts = false;
        UserKnownHostsFile = "~/.ssh/known_hosts";
        ControlMaster = "no";
        ControlPath = "~/.ssh/master-%r@%n:%p";
        ControlPersist = "no";
      };
      github = {
        Hostname = "github.com";
        User = "git";
      };
      gitlab = {
        Hostname = "gitlab.com";
        User = "git";
      };
      rossmann = {
        Hostname = "192.168.8.247"; # via tailscale
        User = "pbzweihander";
      };
    };
  };
}
