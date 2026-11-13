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
assert foreground_terminal() != f"tty{BOOTVINE_TERMINAL}", "tty12 stays in front of the emergency shell"
assert "Initrd" not in shown, f"bootvine's frame is left under the emergency shell:\n{shown}"
recording.stop()
