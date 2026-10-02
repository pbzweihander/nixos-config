{ config, ... }:
let
  cacheDir = "${config.home.homeDirectory}/.terraform.d/plugin-cache";
in
{
  # Share provider binaries across workspaces instead of downloading a copy into every .terraform/.
  # Terraform does not create the cache directory itself, so keep a placeholder file in it.
  home.file = {
    ".terraformrc".text = ''
      plugin_cache_dir = "${cacheDir}"
    '';
    ".terraform.d/plugin-cache/.keep".text = "";
  };
}
