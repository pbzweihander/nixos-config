{ pkgs, ... }:
{
  security.rtkit.enable = true;
  services.pipewire.jack.enable = true;
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
    sunvox
  ];
}
