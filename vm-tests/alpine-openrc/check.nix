{
  lib,
  pkgs,
  staticBootvine,
}:
let
  alpineVersion = "3.24.4";
  alpineIso = pkgs.fetchurl {
    url = "https://dl-cdn.alpinelinux.org/alpine/v${lib.versions.majorMinor alpineVersion}/releases/x86_64/alpine-virt-${alpineVersion}-x86_64.iso";
    sha256 = "157d353f8e54252cad2d9812a0d56bb7fa2231fc154a110e1d180d7480ef8b4f";
  };
  kernelArguments = lib.concatStringsSep " " [
    "modules=loop,squashfs,sd-mod,usb-storage"
    "console=ttyS0"
    "apkovl=/dev/sr1:iso9660:bootvine.apkovl.tar.gz"
  ];
in
pkgs.runCommand "alpine-openrc-check"
  {
    nativeBuildInputs = [
      pkgs.qemu_kvm
      pkgs.libarchive
      pkgs.xorriso
    ];
    requiredSystemFeatures = [ "kvm" ];
  }
  ''
    mkdir -p overlay/etc/init.d overlay/etc/local.d overlay/etc/apk overlay/usr/local/bin
    mkdir -p overlay/etc/runlevels/sysinit overlay/etc/runlevels/default overlay-disk
    install -m755 ${staticBootvine}/bin/bootvine overlay/usr/local/bin/bootvine
    sed 's|/usr/bin/bootvine|/usr/local/bin/bootvine|g' ${../../bootvine/system-files/openrc/bootvine} \
      > overlay/etc/init.d/bootvine
    chmod 755 overlay/etc/init.d/bootvine
    install -m755 ${./broken-example} overlay/etc/init.d/broken-example
    install -m755 ${./capture-screen.start} overlay/etc/local.d/capture-screen.start
    ln -s /etc/init.d/bootvine overlay/etc/runlevels/sysinit/bootvine
    ln -s /etc/init.d/broken-example overlay/etc/runlevels/default/broken-example
    ln -s /etc/init.d/local overlay/etc/runlevels/default/local
    echo alpine-base > overlay/etc/apk/world
    touch overlay/etc/.default_boot_services
    tar -C overlay --owner=0 --group=0 -czf overlay-disk/bootvine.apkovl.tar.gz etc usr
    xorriso -as mkisofs -R -o overlay.iso overlay-disk
    bsdtar -xf ${alpineIso} boot/vmlinuz-virt boot/initramfs-virt
    timeout 600 qemu-system-x86_64 \
      -enable-kvm -m 1024 -display none -no-reboot \
      -serial file:serial.log \
      -kernel boot/vmlinuz-virt -initrd boot/initramfs-virt \
      -append "${kernelArguments}" \
      -drive file=${alpineIso},media=cdrom,index=2,readonly=on \
      -drive file=overlay.iso,media=cdrom,index=3,readonly=on
    sed -n '/=== bootvine screen ===/,/=== end of bootvine screen ===/p' serial.log > screen.txt
    cat screen.txt
    grep -q "=== end of bootvine screen ===" screen.txt
    grep -q broken-example screen.txt
    mkdir -p $out
    cp serial.log screen.txt $out/
  ''
