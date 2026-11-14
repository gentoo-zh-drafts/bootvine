{ machine, pkgs, ... }:
{
  name = "bootvine-hibernate-resume";

  nodes.machine =
    { lib, ... }:
    {
      imports = [ machine ];

      boot.resumeDevice = "/dev/vdc";
      swapDevices = lib.mkOverride 0 [
        {
          device = "/dev/vdc";
          options = [ "x-systemd.makefs" ];
        }
      ];

      boot.kernelPackages = pkgs.linuxPackages_6_12;

      powerManagement = {
        powerDownCommands = "systemctl --no-block stop backdoor.service";
        resumeCommands = "systemctl --no-block restart backdoor.service";
      };

      virtualisation = {
        memorySize = 2048;
        qemu.options = [
          "-vga none"
          "-device VGA,xres=1280,yres=800"
          "-chardev socket,id=recording-monitor,path=recording-monitor,server=on,wait=off"
          "-mon chardev=recording-monitor,mode=readline"
        ];
        emptyDiskImages = [ 4096 ];
        useNixStoreImage = true;
        sharedDirectories = lib.mkForce { };
        useEFIBoot = true;
      };
    };

  testScript =
    builtins.readFile ../shared/console.py
    + builtins.readFile ../shared/screen_recording.py
    + builtins.readFile ./check.py;
}
