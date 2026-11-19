{
  perSystem =
    {
      config,
      craneLib,
      craneSourceArgs,
      ...
    }:
    {
      checks.lint-clippy = craneLib.cargoClippy (
        craneSourceArgs
        // {
          cargoArtifacts = config.packages.bootvine-dependencies;
          cargoClippyExtraArgs = "--workspace --all-targets -- --deny warnings";
        }
      );
    };
}
