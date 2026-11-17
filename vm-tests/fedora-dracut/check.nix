{
  lib,
  pkgs,
  staticBootvine,
}:
let
  fedoraRelease = "45";
  fedoraCompose = "1.4";
  fedoraCloudImage = pkgs.fetchurl {
    url = "https://dl.fedoraproject.org/pub/fedora/linux/releases/${fedoraRelease}/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-${fedoraRelease}-${fedoraCompose}.x86_64.qcow2";
    sha256 = "50bdc5e70f6a24f540932d15411c1c6ae9fcfa40fb990d876d83c5cc823b2ad0";
  };
  installMedia = "/run/bootvine-install";
  reportStart = "=== bootvine dracut report ===";
  reportEnd = "=== end of bootvine dracut report ===";
  reportHealth = pkgs.writeText "report-health" ''
    #!/bin/sh
    {
      echo "${reportStart}"
      bootvine health
      echo "health exit status: $?"
      journalctl -b -o cat -u dracut-cmdline-ask.service
      echo "${reportEnd}"
    } > /dev/ttyS0 2>&1
    systemctl poweroff
  '';
  cloudInitUserData = pkgs.writeText "user-data" (
    "#cloud-config\n"
    + builtins.toJSON {
      bootcmd = [
        "mkdir -p ${installMedia}"
        "mount -o ro /dev/disk/by-label/BOOTVINE ${installMedia}"
      ];
      write_files = [
        {
          path = "/etc/dracut.conf.d/hostonly-cmdline.conf";
          content = ''
            hostonly_cmdline="yes"
          '';
        }
      ];
      runcmd = [
        "${installMedia}/bootvine install --initrd dracut"
        "install -m755 ${installMedia}/report-health /var/lib/cloud/scripts/per-boot/report-health"
        "grubby --update-kernel=ALL --args=console=tty0"
        "dracut --force"
      ];
      power_state = {
        mode = "poweroff";
        condition = true;
      };
    }
  );
  cloudInitMetaData = pkgs.writeText "meta-data" (
    builtins.toJSON {
      instance-id = "bootvine-fedora-dracut";
      local-hostname = "bootvine-fedora-dracut";
    }
  );
  bootFedora = logFile: ''
    timeout 900 qemu-system-x86_64 \
      -enable-kvm -m 2048 -smp 2 -display none -no-reboot -nic none \
      -serial file:${logFile} \
      -drive file=disk.qcow2,if=virtio \
      -drive file=seed.iso,media=cdrom,readonly=on \
      -drive file=bootvine.iso,media=cdrom,readonly=on
  '';
in
pkgs.runCommand "fedora-dracut-check"
  {
    nativeBuildInputs = [
      pkgs.qemu_kvm
      pkgs.xorriso
    ];
    requiredSystemFeatures = [ "kvm" ];
  }
  ''
    mkdir seed install-media
    cp ${cloudInitUserData} seed/user-data
    cp ${cloudInitMetaData} seed/meta-data
    xorriso -as mkisofs -R -J -V cidata -o seed.iso seed
    install -m755 ${lib.getExe staticBootvine} install-media/bootvine
    install -m755 ${reportHealth} install-media/report-health
    xorriso -as mkisofs -R -J -V BOOTVINE -o bootvine.iso install-media
    qemu-img create -f qcow2 -b ${fedoraCloudImage} -F qcow2 disk.qcow2 20G
    ${bootFedora "install.log"}
    ${bootFedora "boot.log"}
    sed -n '/${reportStart}/,/${reportEnd}/p' boot.log > report.txt
    cat report.txt
    mkdir -p $out
    cp install.log boot.log report.txt $out/
    grep -q "${reportEnd}" report.txt
    grep -q "health exit status: 0" report.txt
    grep -q "this boot's initrd ran bootvine" report.txt
    grep -q "every unit in the initramfs that reads the console runs before bootvine" report.txt
    grep -q "Finished dracut-cmdline-ask.service" report.txt
  ''
