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
    };
  };
}
