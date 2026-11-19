{
  perSystem =
    {
      lib,
      pkgs,
      rustToolchain,
      ...
    }:
    let
      exportBuiltPathsHook = lib.getExe (
        pkgs.writers.writeNuBin "export-built-paths-hook" {
          makeWrapperArgs = [
            "--add-flags"
            "export-built-paths-hook"
          ];
        } ../tasks/nix-built-paths.nu
      );

      ciScriptShell = pkgs.mkShellNoCC {
        packages = [
          pkgs.nushell
          pkgs.gitMinimal
          pkgs.tea
          pkgs.cachix
        ];
        env.EXPORT_HOOK_PROGRAM = exportBuiltPathsHook;
      };
    in
    {
      devShells = {
        ci = ciScriptShell;
        ci-cargo = pkgs.mkShellNoCC {
          inputsFrom = [ ciScriptShell ];
          packages = [
            rustToolchain
            pkgs.stdenv.cc
            pkgs.release-plz
            pkgs.prek
            pkgs.ast-grep
          ];
          env.EXPORT_HOOK_PROGRAM = exportBuiltPathsHook;
        };
      };
    };
}
