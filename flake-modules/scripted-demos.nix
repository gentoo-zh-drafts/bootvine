{
  perSystem =
    {
      config,
      craneLib,
      craneSourceArgs,
      ...
    }:
    {
      packages.scripted-demos = craneLib.buildPackage (
        craneSourceArgs
        // { cargoArtifacts = config.packages.bootvine-dependencies; }
        // {
          inherit (craneLib.crateNameFromCargoToml { cargoToml = ../scripted-demos/Cargo.toml; })
            pname
            version
            ;
          cargoExtraArgs = "-p scripted-demos";
          doCheck = false;
        }
      );
    };
}
