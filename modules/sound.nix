{ pkgs, ... }:
{
  security.rtkit.enable = true;
  services.pipewire = {
    enable = true;
    audio.enable = true;
    pulse.enable = true;
    jack.enable = true;
    alsa = {
      enable = true;
      support32Bit = true;
    };
  };
  users.groups = {
    audio.members = [ "pbzweihander" ];
    rtkit.members = [ "pbzweihander" ];
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
  environment.systemPackages = with pkgs; [
    ardour
    geonkick
    lsp-plugins
    rtaudio
    surge-XT
  ];
}
