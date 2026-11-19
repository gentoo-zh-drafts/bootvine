{ self, ... }:
{
  perSystem =
    {
      config,
      craneLib,
      craneSourceArgs,
      bootvinePackageParts,
      ...
    }:
    {
      packages = {
        bootvine-dependencies = craneLib.buildDepsOnly (
          craneSourceArgs // { pname = "bootvine-dependencies"; }
        );
        bootvine = craneLib.buildPackage (
          craneSourceArgs
          // { cargoArtifacts = config.packages.bootvine-dependencies; }
          // bootvinePackageParts
          // {
            cargoExtraArgs = "-p bootvine";
          }
        );
        default = config.packages.bootvine;
      };
      checks.build-bootvine = config.packages.bootvine;
    };

  flake.overlays.default = final: _: {
    bootvine = self.packages.${final.stdenv.hostPlatform.system}.bootvine;
  };
}
