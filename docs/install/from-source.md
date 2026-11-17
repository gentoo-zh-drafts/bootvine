# From source

For Fedora, openSUSE, Debian, Ubuntu and any other distribution with systemd as
PID 1. Building needs Rust and a C toolchain, and no library headers.

## Install

1. Install a C toolchain and git with your package manager, then Rust:

   | Distribution | Command |
   | --- | --- |
   | Fedora | `sudo dnf install gcc curl git` |
   | openSUSE | `sudo zypper install gcc curl git` |
   | Debian, Ubuntu | `sudo apt install build-essential curl git` |

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. Build the latest release:

   ```sh
   git clone https://codeberg.org/wvhulle/bootvine
   cd bootvine
   git checkout "$(git describe --tags --abbrev=0)"
   cargo build --release --locked -p bootvine
   ```

3. Copy the binary and the systemd files into place, and enable the units,
   which show stage 2 and the shutdown:

   ```sh
   sudo install -Dm755 target/release/bootvine /usr/bin/bootvine
   sudo install -Dm644 -t /usr/lib/systemd/system \
     bootvine/system-files/systemd/bootvine.service \
     bootvine/system-files/systemd/bootvine-shutdown.service
   sudo install -Dm644 -t /usr/lib/systemd/system/systemd-hibernate-resume.service.d \
     bootvine/system-files/systemd/systemd-hibernate-resume.service.d/bootvine.conf
   sudo systemctl daemon-reload
   sudo systemctl enable bootvine.service bootvine-shutdown.service
   ```

   `sudo target/release/bootvine install` does the same in one go.

4. To show the boot from the initrd as well, add bootvine to the initramfs.
   That needs an initramfs that runs systemd, which Fedora's and openSUSE's
   dracut builds. Debian's and Ubuntu's default, initramfs-tools, runs no
   systemd, so there bootvine shows stage 2 only.

   With dracut, copy the module, enable it and rebuild:

   ```sh
   sudo install -Dm755 bootvine/system-files/dracut/90bootvine/module-setup.sh \
     /usr/lib/dracut/modules.d/90bootvine/module-setup.sh
   echo 'add_dracutmodules+=" bootvine "' | sudo tee /etc/dracut.conf.d/bootvine.conf
   sudo dracut --regenerate-all --force
   ```

   `sudo target/release/bootvine install --initrd dracut` writes the same two
   files. If your boot setup copies the initramfs elsewhere, such as a unified
   kernel image, rebuild it the way that setup does after a kernel update.

5. Reboot.

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
sudo dracut --regenerate-all --force
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

In the checkout, fetch and check out the new release, then repeat steps 2 to 4:
the initramfs keeps its own copy of bootvine until it is rebuilt. Reboot and
run `sudo bootvine health`.

## Uninstall

```sh
sudo systemctl disable bootvine.service bootvine-shutdown.service
sudo rm /usr/bin/bootvine \
  /usr/lib/systemd/system/bootvine.service \
  /usr/lib/systemd/system/bootvine-shutdown.service \
  /usr/lib/systemd/system/systemd-hibernate-resume.service.d/bootvine.conf
sudo rm -r /usr/lib/dracut/modules.d/90bootvine /etc/dracut.conf.d/bootvine.conf
sudo dracut --regenerate-all --force
```

`sudo bootvine uninstall` does the same and prints the rebuild command.
