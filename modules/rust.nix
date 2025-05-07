{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [
    cargo
    cargo-edit
    cargo-sort
    rust-analyzer
  ];
}
