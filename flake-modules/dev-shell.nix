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
        inputsFrom = [ config.packages.bootvine ];
        packages = [
          pkgs.prek
          pkgs.ast-grep
          pkgs.release-plz
          pkgs.git-cliff
        ];
      };
    };
}
