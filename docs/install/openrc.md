# OpenRC

For Alpine, Gentoo, Artix, Devuan and other distributions with
[OpenRC](https://github.com/OpenRC/openrc).

OpenRC runs no service manager in the initramfs, so bootvine shows the boot
from when OpenRC starts. It has no password agent: when `dmcrypt` asks for a
LUKS passphrase, bootvine steps aside to the console the prompt is on and
returns once `dmcrypt` is done. Failed services show without a reason line,
since OpenRC keeps no journal.

## Install

1. Install Rust and a C toolchain with your package manager, for example
   `apk add cargo git build-base` on Alpine.

2. Build the latest release without systemd:

   ```sh
   git clone https://codeberg.org/wvhulle/bootvine
   cd bootvine
   git checkout "$(git describe --tags --abbrev=0)"
   cargo build --release --locked -p bootvine --no-default-features --features openrc
   ```

3. Copy the binary and the init script into place, and add it to the
   `sysinit` runlevel:

   ```sh
   sudo install -Dm755 target/release/bootvine /usr/bin/bootvine
   sudo install -Dm755 bootvine/system-files/openrc/bootvine /etc/init.d/bootvine
   sudo rc-update add bootvine sysinit
   ```

   `sudo target/release/bootvine install` does the same; it detects OpenRC
   from `/run/openrc/softlevel`.

4. Reboot.

## Configure

Write only the keys you want to change to `/etc/bootvine.toml`, and check it;
the [Configuration](../configuration.md) page lists them:

```sh
bootvine check /etc/bootvine.toml
```

To keep the kernel messages from before bootvine starts to errors, boot with
`quiet`.

## Check

```sh
sudo bootvine health
```

It checks the init script, the runlevel and the configuration.

## Upgrade

In the checkout, fetch and check out the new release, then repeat steps 2 and
3.

## Uninstall

```sh
sudo rc-update del bootvine sysinit
sudo rm /usr/bin/bootvine /etc/init.d/bootvine
```
