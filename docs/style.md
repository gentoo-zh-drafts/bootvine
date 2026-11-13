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
