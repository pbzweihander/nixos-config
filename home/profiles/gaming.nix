{
  xdg.desktopEntries = {
    proton-8_0 = {
      name = "Proton 8.0";
      exec = ''
        steam-run env STEAM_COMPAT_CLIENT_INSTALL_PATH=/home/pbzweihander/.local/share/Steam STEAM_COMPAT_DATA_PATH=/home/pbzweihander/.proton "/home/pbzweihander/.local/share/Steam/steamapps/common/Proton 8.0/proton" run %f'';
      terminal = false;
      categories = [ "Game" ];
    };
  };
}
