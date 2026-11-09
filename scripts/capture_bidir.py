#!/usr/bin/env python3
"""Bidirectional PTY capture: emulates mllm's build_query_responses responder.

Answers CSI 6n (identical positions, like a parser ignoring OSC 66 draws),
OSC 10/11 (mllm's hardcoded colors), XTGETTCAP (+r empty), and captures all
child output + child stdin writes for replay through mllm's render path.
"""
import os, pty, select, sys, time, signal, re

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

# emulate mllm: cursor starts at 0,0 -> reply 1;1 (parser ignores OSC66 draws,
# so all three CPR replies are identical, matching mllm behavior)
buf = bytearray()
deadline = time.time() + DURATION
while time.time() < deadline:
    r, _, _ = select.select([fd], [], [], 0.2)
    if not r:
        continue
    try:
        data = os.read(fd, 65536)
    except OSError:
        break
    if not data:
        break
    buf.extend(data)
    chunk = bytes(data)
    responses = []
    if b"\x1b[6n" in chunk:
        responses.append("\x1b[1;1R")
    if b"\x1b]10;?" in chunk:
        responses.append("\x1b]10;rgb:c7c7/c7c7/c7c7\x1b\\")
    if b"\x1b]11;?" in chunk:
        responses.append("\x1b]11;rgb:0000/0000/0000\x1b\\")
    if b"\x1bP+q" in chunk:
        responses.append("\x1bP+r\x1b\\")
    for resp in responses:
        os.write(fd, resp.encode())

os.kill(pid, signal.SIGKILL)
os.waitpid(pid, 0)
with open(OUT, "wb") as f:
    f.write(bytes(buf))
print(f"wrote {len(buf)} bytes to {OUT}")
