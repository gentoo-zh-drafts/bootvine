# Configuration

Every setting has a default, so bootvine needs no configuration. To change
some, set only those keys, in `/etc/bootvine.toml` or, on NixOS, in
`services.bootvine.settings`. Where they go and how they reach the initramfs
is on the page for your system: [NixOS](install/nixos.md#configure),
[Arch Linux](install/arch-linux.md#configure),
[from source](install/from-source.md#configure) or
[OpenRC](install/openrc.md#configure).

## Defaults

`bootvine defaults` prints every key with its default as TOML, and
`bootvine defaults --format nix` prints the same as a Nix attribute set:

```toml
[colors.header]
origin = "white"
separators = "dark-gray"
total-time = "white"

[colors.stages]
branches = "dark-gray"
finished-label = "gray"
finished-time = "green"
unit-count = "gray"
failed-count = "light-red"
current = "white"

[colors.tree]
branches = "dark-gray"
columns = "dark-gray"
target = "cyan"
current-stage = "yellow"
reached-stage = "green"
running-unit = "white"
running-marker = "yellow"
waiting-unit = "gray"
finished-unit = "dark-gray"
finished-marker = "green"
failed-unit = "light-red"
overflow = "gray"

[colors.timeline]
rule = "dark-gray"
title = "gray"
axis = "dark-gray"
milestone = "green"
finished = "green"
finished-label = "gray"
failed = "light-red"
queued-bar = "gray"
running = "yellow"
running-label = "white"

[colors.failures]
rule = "dark-gray"
title = "light-red"
unit = "light-red"
reason = "white"
log = "gray"
overflow = "dark-gray"

[colors.footer]
rule = "dark-gray"
done = "green"
running = "yellow"
waiting = "gray"
failed = "light-red"
no-failures = "gray"

[colors.password]
frame = "yellow"
message = "white"
hint = "dark-gray"
checking = "gray"
rejected = "light-red"
backdrop = "dark-gray"

[timing]
keep-finished-units-for = "2s"
hide-runs-shorter-than = "1s"
keep-final-frame-for = "1s"
wait-for-display-manager = "10s"
```

## Colours

Each `[colors.*]` table covers one part of the screen:

| Table | Part |
| --- | --- |
| `header` | the top line with the origin and total time |
| `stages` | the finished stages, such as the initrd |
| `tree` | the unit tree |
| `timeline` | the timeline of the slowest units |
| `failures` | the failed units with their reason and last journal lines |
| `footer` | the tallies of done, running, waiting and failed units |
| `password` | the passphrase popup |

Values are the 16 console colour names, such as `yellow`, `light-yellow`,
`gray` and `dark-gray`. How those 16 colours look is the console palette, set
per system as described on its install page.

## Timing

| Key | Meaning |
| --- | --- |
| `keep-finished-units-for` | How long a finished unit stays in the tree, faded, before it leaves. |
| `hide-runs-shorter-than` | Runs shorter than this get no timer and no timeline bar. |
| `keep-final-frame-for` | How long the last frame stays before the console is handed back. |
| `wait-for-display-manager` | When a display manager is starting, how long to wait for it to switch to its own terminal instead, so the console's status lines never show in between. |

## Early kernel messages

bootvine silences the kernel console while it runs, but kernel messages from
before it starts still reach the screen. To keep those to errors, lower the
console log level: the install page of your system shows how.

## Running it by hand

The units run bootvine with its defaults. Run by hand, it takes these options:

| Option | Meaning |
| --- | --- |
| `--config <path>` | Read the settings from another file than `/etc/bootvine.toml`. |
| `--hold <duration>` | Override `timing.keep-final-frame-for`, e.g. `3s`. |
| `--target <unit>` | Root the tree at this unit instead of the default target. |
| `--user` | Watch your user service manager instead of PID 1. |
| `--shutdown` | Follow a shutdown instead of a boot. |

[Development](development.md#tests) shows how to watch your user manager
start a set of sample units this way.
