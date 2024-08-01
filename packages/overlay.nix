final: prev:

{
  opentrack = prev.opentrack.overrideAttrs (old: {
    version = "2023.3.0";
    src = final.fetchFromGitHub {
      owner = "opentrack";
      repo = "opentrack";
      rev = "opentrack-2023.3.0";
      hash = "sha256-C0jLS55DcLJh/e5yM8kLG7fhhKvBNllv5HkfCWRIfc4=";
    };
    nativeBuildInputs = old.nativeBuildInputs
      ++ (with final; [ wine64Packages.base pkgsi686Linux.glibc ]);
    cmakeFlags = old.cmakeFlags ++ [ "-DSDK_WINE=ON" ];
  });
}
