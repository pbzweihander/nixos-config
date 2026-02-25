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
        sha256 = "sha256:1rkqq8mn7c3w9183ypyd5padw1wlfbj3kfan14x558x3z128hn4w";
      })
    ];
  };
}
