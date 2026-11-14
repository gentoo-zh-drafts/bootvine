{
  pkgs ? import <nixpkgs> { },
}:
{
  bootvine = pkgs.callPackage ./nix/package.nix { };
  overlay = import ./nix/overlay.nix;
  nixosModule = ./nix/nixos-module.nix;
}
