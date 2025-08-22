{ pkgs, ... }:
{
  programs = {
    fish.enable = true;
  };

  users.users.pbzweihander = {
    isNormalUser = true;
    description = "Kangwook Lee";
    extraGroups = [
      "wheel"
      "dialout"
      "audio"
      "rtkit"
    ];
    shell = pkgs.fish;
    uid = 1000;
    createHome = true;
    openssh.authorizedKeys.keyFiles = [ ../authorized_keys ];
  };

  security.pam.loginLimits = [
    {
      domain = "@audio";
      item = "memlock";
      type = "-";
      value = "unlimited";
    }
    {
      domain = "@audio";
      item = "rtprio";
      type = "-";
      value = "99";
    }
  ];
}
