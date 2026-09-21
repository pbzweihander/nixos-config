{ pkgs, lib, ... }:
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
        sha256 = "sha256:018n3rjxslx7my6w1chbjiz3c2ck7nyqzdcdbvp0s2j63xprncsh";
      })
    ];
  };
}
