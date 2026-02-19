{
  buildGoModule,
  fetchFromGitHub,
  lib,
}:
buildGoModule (finalAttrs: {
  pname = "helix-assist";
  version = "2dbe250";
  src = fetchFromGitHub {
    owner = "pbzweihander";
    repo = "helix-assist";
    rev = "2dbe25075cce09322d1f2d2ec70fe6d4053d5ecd";
    hash = "sha256-ILg8HG2Ok2Fv1oIS4EBm+lSLAyVlqHgxv5EPZLgJWac=";
  };
  vendorHash = null;
  subPackages = [ "cmd/helix-assist" ];
  meta = {
    description = "Code assistant language server for Helix with support for OpenAI/Anthropic ";
    homepage = "https://github.com/leona/helix-assist";
    license = lib.licenses.mit;
    maintainers = [ ];
  };
})
