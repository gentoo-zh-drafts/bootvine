# Packaging

`bootvine install --destdir <dir> --no-enable` stages the binary and the
service files under `<dir>` instead of the root, without `sudo` or
`systemctl`. It combines with `--initrd` and `--prefix`; the units' `ExecStart`
follows `--prefix`, while the initramfs hooks expect the default `/usr`.

Stage both initramfs hooks, so the package works with mkinitcpio and dracut
alike:

```sh
bootvine install --destdir "$pkgdir" --no-enable --initrd mkinitcpio
bootvine install --destdir "$pkgdir" --no-enable --initrd dracut
```

For an OpenRC package, build with `--no-default-features --features openrc` and
pass `--service-manager openrc`.

Leave enabling the units to the user, or to the distribution's presets:
`bootvine.service` is wanted by `sysinit.target`, `bootvine-shutdown.service`
by `shutdown.target`.
