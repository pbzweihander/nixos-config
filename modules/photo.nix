{ config, ... }:
{
  environment.systemPackages = [
    config.multiverse.instance.fast.tip.darktable
  ];
}
