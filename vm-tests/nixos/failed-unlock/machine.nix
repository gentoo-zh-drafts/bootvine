{ lib, pkgs, ... }:
let
  secretDisk = import ../encrypted-volume/secret-disk.nix { inherit pkgs; };
  unlockUnit = "systemd-cryptsetup@secret.service";
in
{
  boot.initrd = {
    luks.devices = lib.mkVMOverride {
      secret = {
        device = "/dev/disk/by-uuid/${secretDisk.uuid}";
        crypttabExtraOpts = [ "tries=2" ];
      };
    };
    systemd.targets.initrd = {
      requires = [ unlockUnit ];
      after = [ unlockUnit ];
    };
  };

  virtualisation.vmVariant.virtualisation.qemu.drives = [ secretDisk.qemuDrive ];
}
