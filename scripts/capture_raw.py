#!/usr/bin/env python3
"""Capture raw PTY bytes of a TUI session to a file."""
import os, pty, select, sys, time, signal

CMD = sys.argv[1:]
OUT = os.environ["CAPTURE_OUT"]
DURATION = float(os.environ.get("CAPTURE_SECS", "12"))
ROWS = int(os.environ.get("CAPTURE_ROWS", "30"))
COLS = int(os.environ.get("CAPTURE_COLS", "60"))

pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.environ["COLUMNS"] = str(COLS)
    os.environ["LINES"] = str(ROWS)
    os.chdir(os.environ.get("CAPTURE_CWD", "."))
    os.execvp(CMD[0], CMD)

import fcntl, termios, struct
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

buf = bytearray()
deadline = time.time() + DURATION
while time.time() < deadline:
    r, _, _ = select.select([fd], [], [], 0.2)
    if r:
        try:
            data = os.read(fd, 65536)
        except OSError:
            break
        if not data:
            break
        buf.extend(data)

os.kill(pid, signal.SIGKILL)
os.waitpid(pid, 0)
with open(OUT, "wb") as f:
    f.write(bytes(buf))
print(f"wrote {len(buf)} bytes to {OUT}")
