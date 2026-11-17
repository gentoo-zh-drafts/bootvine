import fcntl
import os
import signal
import struct
import sys
import time

VT_SETMODE = 0x5602
VT_RELDISP = 0x5605
VT_ACTIVATE = 0x5606
VT_WAITACTIVE = 0x5607
VT_AUTO = 0
VT_PROCESS = 1
SESSION_TERMINAL = int(sys.argv[1])
TEARDOWN_DELAY_SECONDS = float(sys.argv[2])
release_requests = []
stop_requests = []

session_terminal = os.open(f"/dev/tty{SESSION_TERMINAL}", os.O_RDWR | os.O_NOCTTY)


def set_switch_mode(switch_mode):
    fcntl.ioctl(
        session_terminal,
        VT_SETMODE,
        struct.pack("bbhhh", switch_mode, 0, signal.SIGUSR1, signal.SIGUSR2, 0),
    )


def record_release_request(signal_number, frame):
    release_requests.append(time.monotonic())
    print(f"release requested ({len(release_requests)} so far)", flush=True)


def tear_down_session():
    print("restoring automatic switching like a session teardown", flush=True)
    set_switch_mode(VT_AUTO)
    try:
        fcntl.ioctl(session_terminal, VT_RELDISP, 1)
        print("released the terminal", flush=True)
    except OSError as release_error:
        print(f"releasing the terminal failed: {release_error}", flush=True)


def record_stop_request(signal_number, frame):
    stop_requests.append(time.monotonic())
    print("stop requested, tearing the session down shortly", flush=True)


def acknowledge_acquire(signal_number, frame):
    print("acquire signalled", flush=True)


signal.signal(signal.SIGUSR1, record_release_request)
signal.signal(signal.SIGUSR2, acknowledge_acquire)
signal.signal(signal.SIGTERM, record_stop_request)
fcntl.ioctl(session_terminal, VT_ACTIVATE, SESSION_TERMINAL)
fcntl.ioctl(session_terminal, VT_WAITACTIVE, SESSION_TERMINAL)
set_switch_mode(VT_PROCESS)
print(f"holding tty{SESSION_TERMINAL} in VT_PROCESS mode", flush=True)
while not stop_requests:
    signal.pause()
time.sleep(TEARDOWN_DELAY_SECONDS)
tear_down_session()
