{
  pkgs,
  ...
}:
{
  environment.systemPackages = with pkgs; [
    naps2
  ];

  hardware.sane = {
    enable = true;
    extraBackends = [
      (pkgs.epsonscan2.override {
        withNonFreePlugins = true;
        withGui = false;
      })
    ];
  };

  users.users."pbzweihander".extraGroups = [
    "scanner"
    "lp"
  ];
}
