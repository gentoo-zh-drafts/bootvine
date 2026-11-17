{ lib, sampleMachines, ... }:
{
  perSystem =
    {
      config,
      pkgs,
      builds,
      rustToolchain,
      ...
    }:
    {
      apps = import ../nix/task-apps.nix {
        inherit pkgs;
        mkApp = drv: {
          type = "app";
          program = lib.getExe drv;
        };
        bootvinePackage = builds.bootvine;
        scriptedDemos = builds.scripted-demos;
        vmCheckOutputs = pkgs.linkFarm "vm-check-outputs" (
          lib.mapAttrs (machineName: _: config.checks."vm-${machineName}") sampleMachines.machines
        );
        cargoToolchain = rustToolchain;
        preCommitInputs = [
          rustToolchain
          pkgs.ast-grep
        ];
        extraReleaseInputs = [ pkgs.stdenv.cc ];
      };
    };
}
