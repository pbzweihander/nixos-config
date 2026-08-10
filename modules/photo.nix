{ pkgs, inputs, ... }:
let
  mv = inputs.multiverse.multiverse.${pkgs.stdenv.hostPlatform.system};
in
{
  environment.systemPackages = [
    mv.tip.darktable
  ];
}
