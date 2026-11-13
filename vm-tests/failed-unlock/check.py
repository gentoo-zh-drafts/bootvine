def answered_prompts():
    return bootvine_journal().count("answered a password prompt")


def pending_prompt():
    listing = machine.succeed("ls /run/systemd/ask-password").split()
    return next((name for name in listing if name.startswith("ask.")), "")


def foreground_screen():
    attributed, characters = machine.succeed(
        "base64 -w0 /dev/vcsa; echo; base64 -w0 /dev/vcsu"
    ).split()
    columns = base64.b64decode(attributed)[1]
    return as_rows(base64.b64decode(characters).decode("utf-32-le"), columns)


def in_emergency_mode():
    return machine.execute("systemctl is-active emergency.service")[1].strip() == "active"


def answer_wrongly_until_emergency(last_try):
    if in_emergency_mode():
        return True
    prompt = pending_prompt()
    if prompt and prompt not in answered_prompt_files and "passphrase" in screen():
        answered_before = answered_prompts()
        machine.send_chars("not the passphrase\n")
        retry(lambda _: answered_prompts() > answered_before, timeout_seconds=30)
        answered_prompt_files.append(prompt)
    return in_emergency_mode()


def emergency_message_shown(last_try):
    shown = foreground_screen()
    if last_try:
        print(shown)
        print(bootvine_journal())
    return "emergency mode" in shown or "root account is locked" in shown


recording = ScreenRecording(machine)
machine.start()
recording.start()

answered_prompt_files = []
retry(lambda _: pending_prompt() != "", timeout_seconds=60)
retry(answer_wrongly_until_emergency, timeout_seconds=120)
assert answered_prompt_files, "emergency mode started before any passphrase was asked"

retry(emergency_message_shown, timeout_seconds=30)
shown = foreground_screen()
journal = bootvine_journal()
print(shown)
print(journal)
bootvine_state = machine.execute("systemctl is-active bootvine.service")[1].strip()
assert bootvine_state != "active", f"bootvine still runs in emergency mode:\n{journal}"
assert_bootvine_in_front()
rows = shown.split("\n")


def first_row_with(*fragments):
    return next(
        (index for index, row in enumerate(rows) if any(fragment in row for fragment in fragments)),
        None,
    )


failure_row = first_row_with("systemd-cryptsetup@secret  ")
journal_row = first_row_with("Passphrase incorrect", "Too many attempts", "Failed to activate")
shell_row = first_row_with("emergency mode", "root account is locked")
assert failure_row is not None, f"the failed unlock is not in the summary:\n{shown}"
assert journal_row is not None, f"the summary misses the unlock's journal lines:\n{shown}"
assert shell_row is not None, f"the emergency shell's message is not on screen:\n{shown}"
assert failure_row < journal_row < shell_row, f"the shell does not start below the summary:\n{shown}"
recording.stop()
