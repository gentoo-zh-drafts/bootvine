{
  perSystem =
    {
      config,
      pkgs,
      craneLib,
      ...
    }:
    {
      devShells.default = craneLib.devShell {
        inputsFrom = [
          config.packages.bootvine
          config.devShells.ci-cargo
        ];
        packages = [
          pkgs.git-cliff
          pkgs.vhs
          pkgs.libwebp
        ];
      };
    };
}
