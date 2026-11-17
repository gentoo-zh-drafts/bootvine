# NixOS

The NixOS module installs bootvine, enables its units and adds it to the
systemd initrd.

## Install

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

   The initrd is a systemd initrd by default since NixOS 26.05. On NixOS
   25.11 and earlier, also set `boot.initrd.systemd.enable = true` to show the
   boot from the initrd.

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

## Configure

Set only the keys you want to change under `services.bootvine.settings`; the
[Configuration](../configuration.md) page lists them. The module writes them to
`/etc/bootvine.toml` in the real root and the initrd, and the rebuild fails if
`bootvine check` rejects them:

```nix
services.bootvine.settings = {
  colors.tree.running-marker = "light-yellow";
  timing.keep-final-frame-for = "500ms";
};
```

`bootvine defaults --format nix` prints every key with its default, ready to
paste.

To change how the 16 console colours look, set `console.colors`. To keep the
kernel messages from before bootvine starts to errors, set:

```nix
boot.consoleLogLevel = 3;
```

## Check

After a reboot:

```sh
sudo bootvine health
```

## Upgrade

Update the flake input (`nix flake update bootvine`) or the pinned source, then
rebuild and reboot; the rebuild also rebuilds the initrd.

## Uninstall

Remove `services.bootvine.enable` and the module import, then rebuild.
