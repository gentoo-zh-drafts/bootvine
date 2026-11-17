# Install

Installing bootvine takes up to three steps: put the binary and its service
files in place, enable them, and, to show the boot from the initrd as well, add
bootvine to the initramfs. Afterwards, `bootvine health` checks each step.

| Distribution | Get bootvine | Usual initramfs |
| --- | --- | --- |
| NixOS | [the NixOS module](#nixos), which does every step | systemd initrd |
| Arch Linux | [`pacman -S bootvine`](#arch-linux) | [mkinitcpio](#mkinitcpio) |
| EndeavourOS | [`pacman -S bootvine`](#arch-linux) | [dracut](#dracut) |
| Fedora, openSUSE | [from source](#from-source) | [dracut](#dracut) |
| Debian, Ubuntu | [from source](#from-source) | [initramfs-tools](#initramfs-tools): stage 2 only |
| Alpine, Gentoo, Artix, Devuan | [from source, for OpenRC](#openrc) | none |

## NixOS

1. Add the flake to your system flake's inputs and pass its module in:

   ```nix
   {
     inputs.bootvine = {
       url = "git+https://codeberg.org/wvhulle/bootvine";
       inputs.nixpkgs.follows = "nixpkgs";
     };

     outputs = { nixpkgs, bootvine, ... }: {
       nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
         modules = [
           bootvine.nixosModules.default
           ./configuration.nix
         ];
       };
     };
   }
   ```

2. Enable it in `configuration.nix`:

   ```nix
   {
     services.bootvine.enable = true;
   }
   ```

   The initrd is a systemd initrd by default since NixOS 26.05, and the only
   kind after it. On NixOS 25.11 and earlier, also set
   `boot.initrd.systemd.enable = true` to show the boot from the initrd.

3. Rebuild and reboot:

   ```sh
   sudo nixos-rebuild switch --flake .#my-host
   ```

The flake builds bootvine with [crane](https://github.com/ipetkov/crane), so a
rebuild reuses the compiled dependencies; they are also in the `wvhulle`
[Cachix](https://www.cachix.org) cache the flake offers.

### Without flakes

Import the module from a tarball, or from a source pinned with
[npins](https://github.com/andir/npins) or [niv](https://github.com/nmattia/niv).
It builds bootvine with your system's nixpkgs:

```nix
{
  imports = [
    "${fetchTarball "https://codeberg.org/wvhulle/bootvine/archive/main.tar.gz"}/nix/nixos-module.nix"
  ];

  services.bootvine.enable = true;
}
```

Then run `sudo nixos-rebuild switch`. The repository's `default.nix` also
exposes `bootvine`, `overlay` and `nixosModule`.

## 1. Get bootvine

### Arch Linux

The [`bootvine`](https://archlinux.org/packages/extra/x86_64/bootvine/) package
in the official repositories installs the binary, both systemd units and the
mkinitcpio hook. It does not include the dracut module yet; with dracut, add it
[by hand](#dracut).

```sh
sudo pacman -S bootvine
```

### From source

Building needs Rust and a C toolchain, and no library headers.

1. Install a C toolchain with your package manager, then Rust:

   | Distribution | C toolchain |
   | --- | --- |
   | Debian, Ubuntu | `sudo apt install build-essential curl git` |
   | Fedora | `sudo dnf install gcc curl git` |
   | Arch | `sudo pacman -S base-devel git` |
   | openSUSE | `sudo zypper install gcc curl git` |

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. Build bootvine from a checkout of the latest release:

   ```sh
   git clone https://codeberg.org/wvhulle/bootvine
   cd bootvine
   git checkout "$(git describe --tags --abbrev=0)"
   cargo build --release --locked -p bootvine
   ```

3. Copy the binary and the systemd files into place:

   ```sh
   sudo install -Dm755 target/release/bootvine /usr/bin/bootvine
   sudo install -Dm644 -t /usr/lib/systemd/system \
     bootvine/system-files/systemd/bootvine.service \
     bootvine/system-files/systemd/bootvine-shutdown.service
   sudo install -Dm644 -t /usr/lib/systemd/system/systemd-hibernate-resume.service.d \
     bootvine/system-files/systemd/systemd-hibernate-resume.service.d/bootvine.conf
   sudo systemctl daemon-reload
   ```

   `bootvine install` does the same in one go: it copies the running binary
   and the service files it carries, and enables the units. With
   `--initrd dracut` or `--initrd mkinitcpio` it also copies that initramfs
   hook.

## 2. Enable the units

```sh
sudo systemctl enable bootvine.service bootvine-shutdown.service
```

`bootvine.service` follows the boot, `bootvine-shutdown.service` the shutdown.
This is enough to show stage 2 and the shutdown.

## 3. Show the boot from the initrd

The initramfs must run systemd. Find out which tool builds yours: Arch uses
mkinitcpio by default; Fedora, openSUSE and EndeavourOS use dracut.

### mkinitcpio

1. The Arch package already installed the hook. From source, copy it:

   ```sh
   sudo install -Dm644 bootvine/system-files/mkinitcpio/install/sd-bootvine \
     /usr/lib/initcpio/install/sd-bootvine
   ```

2. Add `sd-bootvine` after `systemd` in `HOOKS` in `/etc/mkinitcpio.conf`,
   for example:

   ```sh
   HOOKS=(base systemd autodetect microcode modconf kms keyboard sd-vconsole block sd-bootvine filesystems fsck)
   ```

   An initramfs built with the `udev` hook instead of `systemd` runs no
   systemd, so bootvine only shows stage 2 there.

3. Rebuild the images:

   ```sh
   sudo mkinitcpio -P
   ```

### dracut

1. Copy the module, from a checkout of the [release](#from-source) that matches
   your bootvine, and enable it:

   ```sh
   sudo install -Dm755 bootvine/system-files/dracut/90bootvine/module-setup.sh \
     /usr/lib/dracut/modules.d/90bootvine/module-setup.sh
   echo 'add_dracutmodules+=" bootvine "' | sudo tee /etc/dracut.conf.d/bootvine.conf
   ```

   With bootvine installed, `bootvine install --initrd dracut` writes the
   same two files.

2. Rebuild the initramfs of every installed kernel:

   ```sh
   sudo dracut --regenerate-all --force
   ```

   If your boot setup copies the initramfs elsewhere, such as a unified kernel
   image or limine's boot entries, rebuild it the way that setup does after a
   kernel update.

dracut runs a few units that read the console, such as
`dracut-cmdline-ask.service` for `rd.cmdline=ask`; bootvine starts after them,
so their prompts come first.

### initramfs-tools

initramfs-tools, the default on Debian and Ubuntu, runs no systemd, so bootvine
starts in stage 2 there.

## 4. Check

Reboot, then run:

```sh
sudo bootvine health
```

It checks that the installed units match this bootvine and are enabled, that
`/etc/bootvine.toml` is valid, that the initramfs hooks are in place, that this
boot's initrd ran this version of bootvine, and, with dracut, that every unit in
the initramfs that reads the console runs before bootvine. Each problem comes
with the command that fixes it, and the exit status is non-zero while one is
left:

```
✓ bootvine.service matches bootvine 0.1.4
✓ bootvine.service is enabled
✓ bootvine-shutdown.service matches bootvine 0.1.4
✓ bootvine-shutdown.service is enabled
✓ no /etc/bootvine.toml, using the defaults
✓ dracut has bootvine's module
✗ this boot's initrd ran a bootvine older than 0.1.4
    → rebuild the initramfs (dracut --force, mkinitcpio -P, or nixos-rebuild) and reboot
```

## Upgrading

The initramfs keeps its own copy of bootvine and its unit, so an upgrade takes
effect in the initrd only after rebuilding it:

1. Update bootvine: `sudo pacman -Syu`, `nixos-rebuild`, or from source a new
   checkout followed by [step 3 of From source](#from-source).
2. Rebuild the initramfs as in [step 3](#3-show-the-boot-from-the-initrd).
3. Reboot and run `sudo bootvine health`.

## OpenRC

On Alpine, Gentoo, Artix or Devuan with
[OpenRC](https://github.com/OpenRC/openrc), build without systemd, from a
checkout as in [From source](#from-source):

```sh
cargo build --release --locked -p bootvine --no-default-features --features openrc
sudo install -Dm755 target/release/bootvine /usr/bin/bootvine
sudo install -Dm755 bootvine/system-files/openrc/bootvine /etc/init.d/bootvine
sudo rc-update add bootvine sysinit
```

`bootvine install` does the same; it detects OpenRC from
`/run/openrc/softlevel`. OpenRC runs no service manager in the initramfs, so
there is no initrd hook, and it has no password agent: when `dmcrypt` asks for a
LUKS passphrase, bootvine steps aside to the console the prompt is on and
returns once `dmcrypt` is done. Failed services show without a reason line,
since OpenRC keeps no journal. `bootvine health` checks the init script and the
runlevel.

## Uninstall

Remove what the steps above added, then rebuild the initramfs:

- With the Arch package: `sudo pacman -R bootvine`, and the dracut files if
  you added them.
- From source: `sudo systemctl disable bootvine.service
  bootvine-shutdown.service`, then delete `/usr/bin/bootvine`, the two units
  and the `systemd-hibernate-resume.service.d/bootvine.conf` drop-in under
  `/usr/lib/systemd/system`, and the hook files:
  `/usr/lib/initcpio/install/sd-bootvine` (and `sd-bootvine` in `HOOKS`), or
  `/usr/lib/dracut/modules.d/90bootvine` and
  `/etc/dracut.conf.d/bootvine.conf`. `bootvine uninstall` does this and prints
  the rebuild command.
- On OpenRC: `sudo rc-update del bootvine sysinit`, then delete
  `/usr/bin/bootvine` and `/etc/init.d/bootvine`.
- On NixOS: remove `services.bootvine.enable` and rebuild.

## Packaging

`bootvine install --destdir <dir> --no-enable` stages the files under `<dir>`
instead of the root, without `sudo` or `systemctl`, and combines with
`--initrd` and `--prefix`. The units' `ExecStart` follows `--prefix`; the
initrd hooks expect the default `/usr`. Run it once per initramfs generator so
the package works with both:

```sh
bootvine install --destdir "$pkgdir" --no-enable --initrd mkinitcpio
bootvine install --destdir "$pkgdir" --no-enable --initrd dracut
```

Pass `--service-manager openrc` when staging an OpenRC package.
