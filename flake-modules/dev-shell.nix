{
  perSystem =
    {
      config,
      lib,
      pkgs,
      craneLib,
      ...
    }:
    let
      exportBuiltPathsHook = pkgs.writers.writeNuBin "export-built-paths-hook" {
        makeWrapperArgs = [
          "--add-flags"
          "export-built-paths-hook"
        ];
      } ../tasks/nix-built-paths.nu;
    in
    {
      devShells.default = craneLib.devShell {
        inputsFrom = [ config.packages.bootvine ];
        packages = [
          pkgs.prek
          pkgs.ast-grep
          pkgs.release-plz
          pkgs.git-cliff
          pkgs.nushell
          pkgs.gitMinimal
          pkgs.tea
          pkgs.cachix
          pkgs.vhs
          pkgs.libwebp
        ];
        EXPORT_HOOK_PROGRAM = lib.getExe exportBuiltPathsHook;
      };
    };
}
