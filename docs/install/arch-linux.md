# Arch Linux

For Arch Linux and distributions built on its repositories, such as
EndeavourOS and Manjaro.

## Install

1. Install the [`bootvine`](https://archlinux.org/packages/extra/x86_64/bootvine/)
   package. It brings the binary, both systemd units and the mkinitcpio hook:

   ```sh
   sudo pacman -S bootvine
   ```

2. Enable the units, which show stage 2 and the shutdown:

   ```sh
   sudo systemctl enable bootvine.service bootvine-shutdown.service
   ```

3. To show the boot from the initrd as well, add bootvine to the initramfs.
   Arch builds it with mkinitcpio; EndeavourOS with dracut.

   **mkinitcpio:** add `sd-bootvine` after `systemd` in `HOOKS` in
   `/etc/mkinitcpio.conf`, for example:

   ```sh
   HOOKS=(base systemd autodetect microcode modconf kms keyboard sd-vconsole block sd-bootvine filesystems fsck)
   ```

   then rebuild the images:

   ```sh
   sudo mkinitcpio -P
   ```

   An initramfs with the `udev` hook instead of `systemd` runs no systemd, so
   bootvine shows only stage 2 there.

   **dracut:** the package has no dracut module yet. Copy it from a checkout
   of the release that matches your package, enable it and rebuild:

   ```sh
   git clone --branch "v$(pacman -Q bootvine | cut -d' ' -f2 | cut -d- -f1)" https://codeberg.org/wvhulle/bootvine
   sudo install -Dm755 bootvine/bootvine/system-files/dracut/90bootvine/module-setup.sh \
     /usr/lib/dracut/modules.d/90bootvine/module-setup.sh
   echo 'add_dracutmodules+=" bootvine "' | sudo tee /etc/dracut.conf.d/bootvine.conf
   sudo dracut --regenerate-all --force
   ```

   `sudo bootvine install --initrd dracut` writes the same two files. If your
   boot setup copies the initramfs elsewhere, such as limine's boot entries or
   a unified kernel image, rebuild it the way that setup does after a kernel
   update.

4. Reboot.

## Configure

Write only the keys you want to change to `/etc/bootvine.toml`; the
[Configuration](../configuration.md) page lists them:

```toml
[colors.tree]
running-marker = "light-yellow"

[timing]
keep-final-frame-for = "500ms"
```

Check the file, then rebuild the initramfs, which holds a copy of it:

```sh
bootvine check /etc/bootvine.toml
sudo mkinitcpio -P  # or: sudo dracut --regenerate-all --force
```

At boot, bootvine ignores an invalid file and uses the defaults.

To change how the 16 console colours look, set the `vt.default_red`,
`vt.default_grn` and `vt.default_blu` kernel parameters. To keep the kernel
messages from before bootvine starts to errors, boot with `quiet`.

## Check

After a reboot:

```sh
sudo bootvine health
```

## Upgrade

`sudo pacman -Syu` updates bootvine. The initramfs keeps its own copy until it
is rebuilt, so rebuild it: `sudo mkinitcpio -P`, or with dracut, update the
module from the new release as in step 3 and run
`sudo dracut --regenerate-all --force`. Then reboot and run
`sudo bootvine health`.

## Uninstall

```sh
sudo pacman -R bootvine
```

With dracut, also delete `/usr/lib/dracut/modules.d/90bootvine` and
`/etc/dracut.conf.d/bootvine.conf`. Remove `sd-bootvine` from `HOOKS`, then
rebuild the initramfs.
