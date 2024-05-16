{ pkgs, ... }:

{
  services.pipewire = {
    enable = true;
    audio.enable = true;
    pulse.enable = true;
    alsa.enable = true;
  };

  environment.systemPackages = with pkgs; [
    pulseaudioFull
  ];
}

