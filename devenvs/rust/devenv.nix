{ inputs, ... }:

with inputs;

{
  languages.rust = {
    enable = true;
    toolchain = if builtins.pathExists ./rust-toolchain.toml
    || builtins.pathExists ./rust-toolchain then
      fenix.packages.x86_64-linux.fromToolchainFile { dir = ./.; }
    else
      fenix.packages.x86_64-linux.stable;

    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" ];
  };

  enterShell = ''
    exec fish
  '';
}
