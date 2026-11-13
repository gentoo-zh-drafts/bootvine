def kernel_console_level():
    return machine.succeed("cut -f1 /proc/sys/kernel/printk").strip()


def manager_show_status():
    return machine.succeed("systemctl show --property=ShowStatus --value").strip()


machine.start()
retry(lambda _: kernel_console_level() == "1", timeout_seconds=60)
retry(lambda _: manager_show_status() == "no", timeout_seconds=60)
saved_level = machine.succeed("cat /run/bootvine/kernel-console-level").strip()
assert saved_level != "1", f"saved the silenced level instead of the original: {saved_level}"

viewer_pid = machine.succeed("systemctl show bootvine --property=MainPID --value").strip()
machine.succeed(f"kill -KILL {viewer_pid}")

retry(lambda _: kernel_console_level() == saved_level, timeout_seconds=30)
retry(lambda _: manager_show_status() == "yes", timeout_seconds=30)
