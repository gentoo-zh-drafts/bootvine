# Code style

- No comments; names and call flow carry the intent.
- Public fields over getters, durations over raw integers, existing crates over
  hand-written formatting.
- Iterator and stream combinators over loops.
- Checked arithmetic over saturating or wrapping; where clamping is the
  meaning, say so in an `#[expect(clippy::disallowed_methods, reason = ...)]`.
- Errors are never dropped silently: no `.ok()` on a `Result`. Expected
  outcomes (a missing file, `EAGAIN`, a unit that is not loaded) become enum
  variants; failures with a recovery path, such as bus errors that trigger a
  reconnect, go up with `?`; anything else is a crash with `expect` saying
  which assumption broke. systemd restores the console through
  `ExecStopPost=bootvine restore-console-state`, so a crash never leaves boot
  messages hidden.
- Enums that name outcomes over `Option` or `bool`, and `From`/`TryFrom` over
  ad-hoc `of`/`from_*` constructors.
- Logs go to the journal through `tracing` with fields (`unit`, `path`,
  `error`) and a constant message.
- No lossy unicode conversions for data; `display()` for text only shown on
  screen.

Clippy runs with the pedantic lints and forbids absolute paths; formatting
needs the nightly toolchain the dev shell provides. See
[Development](development.md#before-committing) for the commands.

## Languages

Each language only where it is the native one for the job:

- Rust for bootvine, and for any logic a user or a check needs, such as
  `bootvine health`.
- Nix for builds, checks and machine definitions; config files are generated
  from Nix values, not written by hand.
- Python only for the test scripts of the NixOS VM checks.
- Nushell for every repository task in `tasks/`, run from the dev shell; they need nothing compiled,
  so CI can run them before the dependencies are built.
- Shell only where the host tool requires it: the dracut module, the
  mkinitcpio hook and the OpenRC init script.
