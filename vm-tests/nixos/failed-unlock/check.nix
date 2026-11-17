{ machine, pkgs }:
let
  secretDisk = import ../encrypted-volume/secret-disk.nix { inherit pkgs; };
in
{
  name = "bootvine-failed-unlock";
  nodes.machine = {
    imports = [
      machine
      ../shared/recorded-display.nix
    ];

    testing.initrdBackdoor = true;
    boot.initrd.systemd.services.panic-on-fail.wantedBy = pkgs.lib.mkForce [ ];

    virtualisation.qemu.drives = [ secretDisk.qemuDrive ];
  };

  testScript =
    builtins.readFile ../shared/console.py
    + builtins.readFile ../shared/screen_recording.py
    + builtins.readFile ./check.py;
}
