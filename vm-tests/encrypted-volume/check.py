recording = ScreenRecording(machine)
machine.start()
recording.start()

wait_for_bootvine_message("asking for a password")
wait_for_screen("passphrase")
assert_text_visible(font_has_512_glyphs=True)
machine.send_chars("not the passphrase\n")
wait_for_bootvine_message("answered a password prompt")
wait_for_bootvine_message("asking for a password", occurrences=2)
wait_for_screen("passphrase")
assert_text_visible(font_has_512_glyphs=True)
machine.send_chars(f"{PASSPHRASE}\n")
machine.wait_for_unit("systemd-cryptsetup@secret.service")
machine.switch_root()
machine.wait_for_unit("multi-user.target")

machine.succeed("test -e /dev/mapper/secret")
journal = bootvine_journal()
print(journal)
assert bootvine_watched("Initrd"), "never ran in the initrd"
assert journal.count("answered a password prompt") == 2, "did not answer both attempts itself"
recording.stop()
