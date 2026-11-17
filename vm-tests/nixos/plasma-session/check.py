def unit_state(unit):
    return machine.succeed(f"systemctl is-active {unit} || true").strip()


machine.start()
machine.wait_until_succeeds("pgrep -u sample-user plasmashell", timeout=300)
machine.wait_until_succeeds("pgrep -u sample-user -f kwin_wayland", timeout=60)
session_terminal = foreground_terminal()
print(f"plasma session in front on {session_terminal}")
print(machine.succeed("loginctl list-sessions; loginctl show-session $(loginctl list-sessions --no-legend | awk '$3 == \"sample-user\" {print $1}') -p VTNr -p Type -p Active -p Class"))
machine.screenshot("plasma-session")
assert session_terminal != f"tty{BOOTVINE_TERMINAL}", "bootvine is still in front of the plasma session"

machine.wait_for_unit("sample-database.service")
print(machine.succeed("systemd-cgls --full --no-pager -u session-1.scope"))
machine.succeed("systemd-cgls --full --no-pager -u session-1.scope | grep -q slow-session-exit")
machine.succeed("systemctl poweroff --no-block")
retry(lambda _: unit_state("sample-database") == "deactivating", timeout_seconds=120)
wait_for_screen("Shutdown", "sample-database")


def bootvine_reached_front(_):
    return foreground_terminal() == f"tty{BOOTVINE_TERMINAL}"


reached_front = True
try:
    retry(bootvine_reached_front, timeout_seconds=30)
except Exception:
    reached_front = False

machine.screenshot("shutdown")
print(machine.succeed("journalctl -b -o short-monotonic -u systemd-logind -u display-manager -u bootvine-shutdown"))
print(bootvine_journal())
terminal_in_front = foreground_terminal()
print(f"foreground terminal during shutdown: {terminal_in_front}")
machine.succeed("touch /run/release-sample-database")
assert reached_front, f"bootvine-shutdown drew its screen but {terminal_in_front} stayed in front"
machine.wait_for_shutdown()
