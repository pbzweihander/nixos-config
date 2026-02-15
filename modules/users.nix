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
    openssh.authorizedKeys.keyFiles = [
      (builtins.fetchurl {
        url = "https://github.com/pbzweihander.keys";
        sha256 = "sha256:1zydkj8f4xpsv2p1vx8kndmpwlhlhxi7l9hmr55n7y5glszn05ci";
      })
    ];
  };
}
