def unit_state(unit):
    return machine.succeed(f"systemctl is-active {unit} || true").strip()


machine.start()
retry(lambda _: unit_state("bootvine") == "inactive", timeout_seconds=120)
machine.wait_until_tty_matches("1", "login: ")

machine.succeed("systemctl start session-terminal-holder")
retry(lambda _: foreground_terminal() == "tty2", timeout_seconds=30)

machine.succeed("systemctl poweroff --no-block")
retry(lambda _: unit_state("sample-database") == "deactivating", timeout_seconds=60)
wait_for_screen("Shutdown", "sample-database")


def bootvine_reached_front(_):
    return foreground_terminal() == f"tty{BOOTVINE_TERMINAL}"


reached_front = True
try:
    retry(bootvine_reached_front, timeout_seconds=15)
except Exception:
    reached_front = False

holder_journal = machine.succeed("journalctl -b -o cat -u session-terminal-holder")
print(holder_journal)
print(machine.succeed("journalctl -b -o cat -u bootvine-shutdown"))
print(bootvine_journal())
terminal_in_front = foreground_terminal()
print(f"foreground terminal during shutdown: {terminal_in_front}")
machine.succeed("touch /run/release-sample-database")
assert "release requested" in holder_journal, "bootvine never asked the session terminal to release"
assert "releasing the terminal failed" in holder_journal, "the session teardown did not lose the switch"
assert reached_front, f"bootvine-shutdown drew its screen but {terminal_in_front} stayed in front"
machine.wait_for_shutdown()
