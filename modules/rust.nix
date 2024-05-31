{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [ rust-analyzer cargo-sort ];
}
