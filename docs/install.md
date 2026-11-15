# Install

bootvine always shows stage 2 and shutdown. To show the boot from the initrd
as well, the initrd has to run systemd.

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
exposes `bootvine`, `overlay` and `nixosModule`; the flake exports the same
overlay as `overlays.default`.

## Arch Linux

Install the package
[`bootvine`](https://archlinux.org/packages/extra/x86_64/bootvine/) from the
official repos:

```sh
sudo pacman -S bootvine
```

Then enable it with:

```sh
sudo systemctl enable bootvine
```

## From source

This works the same on every distribution with systemd: building needs only
Rust and a C toolchain, and no library headers.

1. Install a C toolchain with your package manager, then Rust:

   | Distribution | C toolchain |
   | --- | --- |
   | Debian, Ubuntu | `sudo apt install build-essential curl` |
   | Fedora | `sudo dnf install gcc curl` |
   | Arch | `sudo pacman -S base-devel` |
   | openSUSE | `sudo zypper install gcc curl` |

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. Build bootvine into `~/.cargo/bin`:

   ```sh
   cargo install --locked --git https://codeberg.org/wvhulle/bootvine bootvine
   ```

3. Install it for the system and enable its units. It asks `sudo` itself for
   the copying and `systemctl enable`:

   ```sh
   bootvine install
   ```

   This copies the binary to `/usr/bin/bootvine`, and `bootvine.service` and
   `bootvine-shutdown.service` to `/usr/lib/systemd/system`, which is enough
   for stage 2 and shutdown. The copy in `~/.cargo/bin` is no longer needed;
   `cargo uninstall bootvine` removes it.

4. Optionally, show the boot from the initrd. This needs an initramfs with
   systemd in it, built by dracut or mkinitcpio; initramfs-tools has no
   systemd, so on Debian and Ubuntu releases that use it bootvine starts in
   stage 2 only.

   With [dracut](https://github.com/dracut-ng/dracut-ng):

   ```sh
   bootvine install --initrd dracut
   sudo dracut --force
   ```

   This adds the dracut module in `/usr/lib/dracut/modules.d/90bootvine` and
   enables it in `/etc/dracut.conf.d/bootvine.conf`.

   With [mkinitcpio](https://wiki.archlinux.org/title/Mkinitcpio), install the
   hook in `/usr/lib/initcpio/install/sd-bootvine`, add `sd-bootvine` after
   `systemd` in `HOOKS` in `/etc/mkinitcpio.conf`, and rebuild:

   ```sh
   bootvine install --initrd mkinitcpio
   sudo mkinitcpio -P
   ```

5. Reboot.

## OpenRC

On Alpine, Gentoo, Artix or Devuan with [OpenRC](https://github.com/OpenRC/openrc),
build without systemd:

```sh
cargo install bootvine --no-default-features --features openrc
```

Then install the init script `/etc/init.d/bootvine` and add it to the
`sysinit` runlevel:

```sh
bootvine install
```

`bootvine install` detects OpenRC from `/run/openrc/softlevel`; pass
`--service-manager openrc` when staging with `--destdir`. OpenRC runs no
service manager in the initramfs, so there is no initrd hook, and it has no
password agent: when `dmcrypt` asks for a LUKS passphrase, bootvine steps aside
to the console the prompt is on and returns once `dmcrypt` is done. Failed
services show without a reason line, since OpenRC keeps no journal.

## Packaging

`bootvine install --destdir <dir>` stages the files under `<dir>` instead of
the root, without `sudo` or `systemctl`, and combines with `--initrd` and
`--prefix`. The units' `ExecStart` follows `--prefix`. The initrd hooks expect
the default `/usr`.

## Uninstall

`bootvine uninstall` disables and removes everything `bootvine install` put in
place, with the same `--prefix` and `--destdir`; on OpenRC it also runs
`rc-update del bootvine sysinit`. The current initramfs keeps
its copy of bootvine until you rebuild it, so with an initrd hook installed it
prints the command for that: `sudo dracut --force`, or for mkinitcpio, removing
`sd-bootvine` from `HOOKS` and running `sudo mkinitcpio -P`. On NixOS, remove
`services.bootvine.enable` and rebuild.
