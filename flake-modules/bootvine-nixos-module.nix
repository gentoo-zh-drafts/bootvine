{ self, ... }:
{
  flake.nixosModules = {
    default = self.nixosModules.bootvine;
    bootvine =
      { lib, pkgs, ... }:
      {
        imports = [ ../nix/nixos-module.nix ];
        services.bootvine.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.bootvine;
      };
  };
}
