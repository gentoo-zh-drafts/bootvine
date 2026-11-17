{
  perSystem =
    {
      pkgs,
      craneLib,
      builds,
      ...
    }:
    {
      devShells.default = craneLib.devShell {
        inputsFrom = [ builds.bootvine ];
        packages = [
          pkgs.prek
          pkgs.ast-grep
          pkgs.release-plz
          pkgs.git-cliff
        ];
      };
    };
}
