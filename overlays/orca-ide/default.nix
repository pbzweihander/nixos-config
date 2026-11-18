{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  dpkg,
  makeWrapper,
  alsa-lib,
  at-spi2-atk,
  at-spi2-core,
  atk,
  cairo,
  cups,
  dbus,
  expat,
  fontconfig,
  freetype,
  gdk-pixbuf,
  glib,
  gtk3,
  libdrm,
  libgbm,
  libGL,
  libnotify,
  libpulseaudio,
  libsecret,
  libx11,
  libxcb,
  libxcomposite,
  libxcursor,
  libxdamage,
  libxext,
  libxfixes,
  libxi,
  libxkbcommon,
  libxrandr,
  libxrender,
  libxscrnsaver,
  libxtst,
  musl,
  nspr,
  nss,
  pango,
  pipewire,
  systemd,
  vulkan-loader,
  wayland,
  xdg-utils,
  runtimeShell,
}:
let
  sources = lib.importJSON ./sources.json;
in
stdenv.mkDerivation {
  pname = "orca-ide";
  inherit (sources) version;

  src = fetchurl sources.${stdenv.hostPlatform.system};

  nativeBuildInputs = [
    autoPatchelfHook
    dpkg
    makeWrapper
  ];

  buildInputs = [
    alsa-lib
    at-spi2-atk
    at-spi2-core
    atk
    cairo
    cups
    dbus
    expat
    fontconfig
    freetype
    gdk-pixbuf
    glib
    gtk3
    libdrm
    libgbm
    libx11
    libxcb
    libxcomposite
    libxcursor
    libxdamage
    libxext
    libxfixes
    libxi
    libxkbcommon
    libxrandr
    libxrender
    libxscrnsaver
    libxtst
    musl # Koffi also includes a local musl addon alongside its glibc addon.
    nspr
    nss
    pango
    stdenv.cc.cc.lib
    wayland
  ];

  runtimeDependencies = map lib.getLib [
    libGL
    libnotify
    libpulseaudio
    libsecret
    pipewire
    systemd
    vulkan-loader
    wayland
  ];

  unpackPhase = ''
    runHook preUnpack
    dpkg --fsys-tarfile "$src" | tar --extract --no-same-permissions
    runHook postUnpack
  '';

  dontBuild = true;
  # Remote payloads are copied to other hosts and verified against the template
  # manifest (main/chunks/orcad-artifact-cache-retention-*.js). This includes
  # resources/{orcad-template,agent-browser-*,ripgrep}, and node-runtime when
  # present (orcad/orcad-local-serve-selection-entry.js checks its pinned hash).
  # Preserve their contents, including scripts: no stripping, shebang rewriting,
  # or blanket ELF fixups. Only the local Electron runtime is patched below.
  dontAutoPatchelf = true;
  dontPatchELF = true;
  dontStrip = true;
  dontPatchShebangs = true;

  installPhase = ''
    runHook preInstall

    mkdir -p "$out/share/orca-ide" "$out/bin"
    cp -a opt/Orca/. "$out/share/orca-ide/"
    cp -a usr/share/applications usr/share/icons "$out/share/"

    # Use Chromium's unprivileged user namespace sandbox. A setuid helper cannot
    # work in the Nix store, and disabling that helper does not disable sandboxing.
    rm "$out/share/orca-ide/chrome-sandbox"
    makeWrapper "$out/share/orca-ide/orca-ide" "$out/libexec/orca-ide/orca-ide-gui" \
      --prefix XDG_DATA_DIRS : "$GSETTINGS_SCHEMAS_PATH" \
      --suffix PATH : ${lib.makeBinPath [ xdg-utils ]} \
      --add-flags "--disable-setuid-sandbox" \
      --add-flags "\''${NIXOS_OZONE_WL:+\''${WAYLAND_DISPLAY:+--ozone-platform-hint=auto --enable-wayland-ime}}"

    # Upstream's deb links /usr/bin/orca-ide to the CLI shim (resources/bin/orca-ide),
    # which Orca's own docs call outside Orca terminals; with no arguments that shim
    # only prints help. Launch the GUI when called bare, otherwise run the CLI.
    cat > "$out/bin/orca-ide" <<EOF
    #!${runtimeShell}
    if [ "\$#" -eq 0 ]; then
      exec "$out/libexec/orca-ide/orca-ide-gui"
    fi
    exec "$out/share/orca-ide/resources/bin/orca-ide" "\$@"
    EOF
    chmod +x "$out/bin/orca-ide"
    # Inside Orca terminals `orca` is the same CLI (~/.orca/cli/bin/orca).
    ln -s orca-ide "$out/bin/orca"

    substituteInPlace "$out/share/applications/orca-ide.desktop" \
      --replace-fail "Exec=/opt/Orca/orca-ide" "Exec=$out/libexec/orca-ide/orca-ide-gui" \
      --replace-fail "Icon=orca-ide" "Icon=$out/share/icons/hicolor/512x512/apps/orca-ide.png"

    # Keep resources/package-type = deb: upstream treats installs without a
    # Debian package manager as externally managed and blocks update downloads.
    test "$(cat "$out/share/orca-ide/resources/package-type")" = deb
    patchShebangs "$out/share/orca-ide/resources/bin/orca-ide"

    runHook postInstall
  '';

  postFixup = ''
    # Deliberate allowlist: Electron, its shared libraries/crash handler, local
    # provider/clipboard helpers, and local native Node modules. Never recurse
    # through resources as a whole: the remaining assets are deployment payloads.
    autoPatchelf \
      "$out/share/orca-ide/orca-ide" \
      "$out/share/orca-ide/chrome_crashpad_handler" \
      "$out/share/orca-ide/"*.so* \
      "$out/share/orca-ide/resources/bin" \
      "$out/share/orca-ide/resources/node_modules"
  '';

  passthru.updateScript = ./update.sh;

  meta = {
    description = "IDE for parallel development with coding agents";
    homepage = "https://github.com/stablyai/orca";
    changelog = "https://github.com/stablyai/orca/releases/tag/v${sources.version}";
    license = lib.licenses.mit;
    mainProgram = "orca-ide";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
