{ machine, pkgs }:
let
  secretDisk = import ../encrypted-volume/secret-disk.nix { inherit pkgs; };
in
{
  name = "bootvine-crash-recovery";
  nodes.machine = {
    imports = [ machine ];

    testing.initrdBackdoor = true;

    boot.kernelParams = [ "systemd.show_status=yes" ];

    virtualisation.qemu.drives = [ secretDisk.qemuDrive ];
  };

  testScript = builtins.readFile ./check.py;
}
