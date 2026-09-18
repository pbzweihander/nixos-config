{
  programs.ssh = {
    enable = true;
    enableDefaultConfig = false;
    settings = {
      "*" = {
        ForwardAgent = false;
        AddKeysToAgent = "no";
        Compression = false;
        ServerAliveInterval = 60;
        ServerAliveCountMax = 5;
        HashKnownHosts = false;
        UserKnownHostsFile = "~/.ssh/known_hosts";
        ControlMaster = "auto";
        ControlPath = "~/.ssh/master-%C";
        ControlPersist = "10m";
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
