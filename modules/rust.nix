{ pkgs, inputs, ... }:

with inputs;

{
  nixpkgs.overlays = [ fenix.overlays.default ];
  environment.systemPackages = with pkgs; [
    (fenix.stable.withComponents [
      "cargo"
      "clippy"
      "rust-src"
      "rustc"
      "rustfmt"
    ])
    rust-analyzer-nightly
  ];
}
