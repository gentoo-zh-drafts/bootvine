# Install

Pick the page for your operating system. Each one covers installing,
configuring, checking, upgrading and removing bootvine there.

| Operating system | Page | Shows the initrd with |
| --- | --- | --- |
| NixOS | [NixOS](install/nixos.md) | the systemd initrd |
| Arch Linux, EndeavourOS, Manjaro | [Arch Linux](install/arch-linux.md) | mkinitcpio or dracut |
| Fedora, openSUSE, Debian, Ubuntu and other systemd distributions | [From source](install/from-source.md) | dracut; not initramfs-tools |
| Alpine, Gentoo, Artix, Devuan | [OpenRC](install/openrc.md) | none: stage 2 only |

On every system, `sudo bootvine health` checks the result after a reboot: the
units, the configuration, the initramfs and whether this boot's initrd ran this
version of bootvine. Each problem comes with the command that fixes it.

The settings themselves are the same everywhere; see
[Configuration](configuration.md).
