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
    ];
    shell = pkgs.fish;
    uid = 1000;
    createHome = true;
    openssh.authorizedKeys.keyFiles = [ ../authorized_keys ];
  };
}
