{ self, lib, ... }:
let
  sampleMachines = import ../vm-tests {
    inherit lib;
    bootvineModule = self.nixosModules.bootvine;
  };
in
{
  _module.args = { inherit sampleMachines; };

  flake.nixosConfigurations = lib.mapAttrs' (
    machineName: _:
    lib.nameValuePair "sample-${machineName}" (sampleMachines.nixosSystemFor "x86_64-linux" machineName)
  ) sampleMachines.machines;

  perSystem =
    { pkgs, system, ... }:
    {
      packages.sample-unit-files = pkgs.callPackage ../vm-tests/slow-services/sample-unit-files.nix {
        inherit (sampleMachines) sampleUnits;
      };

      apps = lib.mapAttrs' (
        machineName: machineDescription:
        lib.nameValuePair "sample-vm-${machineName}" {
          type = "app";
          meta.description = "Boot the ${machineName} sample machine in QEMU: ${machineDescription}";
          program = lib.getExe (sampleMachines.nixosSystemFor system machineName).config.system.build.vm;
        }
      ) sampleMachines.machineDescriptions;

      checks = lib.mapAttrs' (machineName: check: lib.nameValuePair "vm-${machineName}" check) (
        sampleMachines.checksFor pkgs
      );
    };
}
