# Development

## Layout

| Folder | Holds |
| --- | --- |
| `bootvine/` | the published crate: the `bootvine` binary and the unit and initramfs files it installs |
| `bootvine-screen/` | the screen as data and pure functions |
| `bootvine-systemd-link/`, `bootvine-openrc-link/` | the systemd and OpenRC sides |
| `scripted-demos/` | the scripted demos and the VHS tapes that record them |
| `vm-tests/` | the VM checks, one folder per operating system: `nixos/` with a folder per sample machine, `alpine/` and `fedora/` |
| `tasks/` | repository tasks, run with `nix run .#<task>` |
| `flake-modules/` | the flake, one [flake-parts](https://flake.parts) module per feature; every file here is loaded |
| `nix/` | what works without flakes, through `default.nix`: the package, the NixOS module and the overlay, plus the source and metadata the flake's build shares with them |

## Setup

Work inside the dev shell (`direnv allow` once, with
[direnv](https://direnv.net)). Version control is
[jj](https://github.com/jj-vcs/jj), not the `git` CLI.

`cargo build` and `cargo test` act on every crate. `bootvine` builds for
systemd by default; add `--features openrc` for both, or
`--no-default-features --features openrc` for OpenRC alone.

## Before committing

```sh
cargo clippy --workspace --all-targets
ast-grep scan --error
cargo fmt --all
nix run .#check-commit-messages -- origin/main..HEAD
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org).
jj runs no git hooks; git users can install them with `prek install`.
See the [code style](style.md) for the conventions these enforce.

## Tests

| What | Command |
| --- | --- |
| Unit tests | `cargo test --workspace` |
| Scripted demos | `cargo run --bin scripted-boot` (also `scripted-shutdown`, `scripted-luks-unlock`) |
| A sample machine in a window | `nix run .#sample-vm-<machine>` |
| One VM check | `nix build .#checks.x86_64-linux.vm-<machine>` |
| Every check | `nix flake check` |

The `vm-*` checks need KVM. Codeberg's CI has none, so run them locally before
pushing a change to the console, the units or the initramfs hooks. Prefix VM
commands with `env -u LD_LIBRARY_PATH` if your shell exports libraries that
clash with [QEMU](https://www.qemu.org).

### Debugging a VM check

- A failed check leaves its log in `nix log <drv>`; the serial console and
  bootvine's journal lines are in it.
- Step through a NixOS check by hand:
  `nix run .#checks.x86_64-linux.vm-<machine>.driverInteractive`, then call
  `test_script()`.
- The checks record the console: frames land in the check's output under
  `recording/`.
- To see bootvine on your own user manager without a VM, start the sample
  units and watch them:

  ```sh
  units=$(nix build --no-link --print-out-paths .#sample-unit-files)
  systemctl --user link --runtime $units/*
  systemctl --user start --no-block bootvine-sample-services.target
  cargo run --bin bootvine -- --user --target bootvine-sample-services.target --hold 3s
  systemctl --user stop $(ls $units)
  systemctl --user disable --runtime $(ls $units)
  ```

### Adding a NixOS VM check

1. Copy a folder under `vm-tests/nixos/`, such as `text-console/`: `machine.nix`
   describes the machine, `check.nix` adapts it for the test, `check.py` is
   the test script.
2. Add its name and a one-line description to `vm-tests/nixos/default.nix`. That
   creates the check `vm-<name>` and the app `sample-vm-<name>`.
3. Write `check.py` with the helpers in `vm-tests/nixos/shared/console.py`:
   `wait_for_screen("Initrd", "unit")` waits for text on bootvine's screen,
   `assert_bootvine_in_front()` checks the console, `bootvine_journal()` and
   `wait_for_bootvine_message()` read the journal.
4. Wait on marker files or screen text, never on `sleep`. Hold a unit at a
   fixed point with an `ExecStart` that waits for a file under `/run`, and
   release it with `machine.succeed("touch /run/<file>")`.
5. End with `bootvine health` where it applies: it fails the check if the
   units, the initramfs or this boot's run are off.

### Checks on other distributions

`vm-tests/alpine/` and `vm-tests/fedora/` boot a distribution image in QEMU and read
a report from the serial console. They use `bootvine-static`, since those images
have no Nix store. To update one, change the release and hash of the image at
the top of its `check.nix`; `nix build` prints the new hash after a mismatch.

## Repository tasks

| Task | Does |
| --- | --- |
| `nix run .#record-screencasts` | re-records the demo animations and the VM check recordings into `recordings/` |
| `nix run .#publish-recordings` | force-pushes `recordings/` to the `recordings` branch, which the README and the guide link to |
| `nix run .#sync-default-config` | rewrites the defaults in [configuration](configuration.md#defaults) from `bootvine defaults` |

## CI and releases

Every push and pull request builds and tests bootvine, runs clippy and the
pre-commit hooks, and evaluates the flake. Builds on `main` go to the `wvhulle`
[Cachix](https://www.cachix.org) cache, which the flake offers as a
substituter.

[release-plz](https://release-plz.dev) keeps a pull request labelled `release`
open that bumps the version and the changelog; merging it publishes the crate,
tags `v<version>` and creates the Codeberg release. Preview it with
`release-plz update`. A weekly job opens a pull request that updates
`Cargo.lock` and `flake.lock`.
