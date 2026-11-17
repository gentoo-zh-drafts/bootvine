{ self, ... }:
{
  perSystem =
    { builds, ... }:
    {
      packages = {
        default = builds.bootvine;
        inherit (builds) bootvine bootvine-dependencies;
      };
      checks.build-bootvine = builds.bootvine;
    };

  flake.overlays.default = final: _: {
    bootvine = self.packages.${final.stdenv.hostPlatform.system}.bootvine;
  };
}
