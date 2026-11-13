import base64
import shlex
from time import monotonic, sleep

BOOTVINE_TERMINAL = 12


def read_terminal():
    attributed, characters = machine.succeed(
        f"base64 -w0 /dev/vcsa{BOOTVINE_TERMINAL}; echo; "
        f"base64 -w0 /dev/vcsu{BOOTVINE_TERMINAL}"
    ).split()
    attributed = base64.b64decode(attributed)
    columns = attributed[1]
    attributes = attributed[5::2]
    text = base64.b64decode(characters).decode("utf-32-le")
    return columns, text, attributes


def as_rows(text, columns):
    return "\n".join(text[start : start + columns] for start in range(0, len(text), columns))


def screen():
    columns, text, _ = read_terminal()
    return as_rows(text, columns)


def colours(attribute, font_has_512_glyphs):
    if font_has_512_glyphs:
        return (attribute >> 1) & 0x0F, (attribute >> 5) & 0x07
    return attribute & 0x0F, (attribute >> 4) & 0x07


def assert_text_visible(font_has_512_glyphs=False):
    columns, text, attributes = read_terminal()

    def is_hidden(character, attribute):
        foreground, background = colours(attribute, font_has_512_glyphs)
        return character != " " and foreground == background

    hidden = "".join(
        "#" if is_hidden(character, attribute) else " "
        for character, attribute in zip(text, attributes)
    )
    assert "#" not in hidden, (
        f"text drawn in its background colour, marked #:\n{as_rows(hidden, columns)}\n{as_rows(text, columns)}"
    )


SCREEN_WAIT_SCRIPT = r"""
use Encode qw(decode);
use IO::Poll qw(POLLPRI POLLERR);
use Time::HiRes qw(time);
my ($device, $timeout, @fragments) = @ARGV;
@fragments = map { decode('UTF-8', $_) } @fragments;
my $deadline = time + $timeout;
open(my $contents, '<:raw', $device) or die "open $device: $!";
my $changes = IO::Poll->new;
$changes->mask($contents => POLLPRI);
while (1) {
    sysseek($contents, 0, 0);
    my $raw = '';
    while (sysread($contents, my $chunk, 65536)) { $raw .= $chunk; }
    my $text = decode('UTF-32LE', $raw);
    exit 0 unless grep { index($text, $_) < 0 } @fragments;
    my $remaining = $deadline - time;
    exit 1 if $remaining <= 0;
    $changes->poll($remaining);
}
"""


PERL_MISSING = 127
SCREEN_READ_INTERVAL_SECONDS = 0.1


def shows_all(fragments):
    text = screen()
    return all(fragment in text for fragment in fragments)


def wait_for_screen_by_reading(fragments, timeout_seconds):
    deadline = monotonic() + timeout_seconds
    while not shows_all(fragments):
        if monotonic() > deadline:
            return False
        sleep(SCREEN_READ_INTERVAL_SECONDS)
    return True


def wait_for_screen(*fragments, timeout_seconds=60):
    arguments = " ".join(
        shlex.quote(argument)
        for argument in [f"/dev/vcsu{BOOTVINE_TERMINAL}", str(timeout_seconds), *fragments]
    )
    status, _ = machine.execute(f"perl -e {shlex.quote(SCREEN_WAIT_SCRIPT)} {arguments}")
    shown = status == 0 or (
        status == PERL_MISSING and wait_for_screen_by_reading(fragments, timeout_seconds)
    )
    if not shown:
        print(screen())
        print(bootvine_journal())
        raise AssertionError(f"never showed {fragments} within {timeout_seconds} s")


def foreground_terminal():
    return machine.succeed("cat /sys/class/tty/tty0/active").strip()


def assert_bootvine_in_front():
    active = foreground_terminal()
    assert active == f"tty{BOOTVINE_TERMINAL}", f"bootvine is not in front: {active}"


def wait_for_bootvine_message(text, occurrences=1, timeout_seconds=60):
    follow = (
        f"count=0; while IFS= read -r line; do "
        f"case \"$line\" in *{shlex.quote(text)}*) count=$((count + 1));; esac; "
        f"[ \"$count\" -ge {occurrences} ] && break; "
        f"done < <(timeout {timeout_seconds} journalctl -b -o cat -f -n all {BOOTVINE_ENTRIES}); "
        f"[ \"$count\" -ge {occurrences} ]"
    )
    status, _ = machine.execute(f"bash -c {shlex.quote(follow)}")
    if status != 0:
        print(bootvine_journal())
        raise AssertionError(f"bootvine never logged {text!r} {occurrences} times within {timeout_seconds} s")


BOOTVINE_ENTRIES = (
    "_SYSTEMD_UNIT=bootvine.service + SYSLOG_IDENTIFIER=bootvine + SYSLOG_IDENTIFIER=@bootvine"
)


def bootvine_journal():
    return machine.succeed(f"journalctl -b -o cat {BOOTVINE_ENTRIES}")


def bootvine_watched(phase):
    return "watching the boot" in machine.succeed(f"journalctl -b -u bootvine 'PHASE={phase}' -o cat")
