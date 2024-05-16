{ pkgs, ...}:

{
  programs = {
    fish.enable = true;
  };

  users.users.pbzweihander = {
    isNormalUser = true;
    description = "Kangwook Lee";
    extraGroups = [ "wheel" ];
    shell = pkgs.fish;
    uid = 1000;
    createHome = true;
  };
}

