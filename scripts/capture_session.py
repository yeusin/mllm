#!/usr/bin/env python3
"""Bidirectional PTY capture of a full opencode mini session with a prompt.
Sends a rich prompt (emoji/markdown) and captures the full render stream."""
import os, pty, select, sys, time, signal

CMD = sys.argv[1:]
OUT = os.environ["CAPTURE_OUT"]
DURATION = float(os.environ.get("CAPTURE_SECS", "60"))
ROWS = int(os.environ.get("CAPTURE_ROWS", "30"))
COLS = int(os.environ.get("CAPTURE_COLS", "60"))
PROMPT = os.environ.get("CAPTURE_PROMPT", "")

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
prompt_sent = False
prompt_at = time.time() + 4
deadline = time.time() + DURATION
while time.time() < deadline:
    if not prompt_sent and PROMPT and time.time() >= prompt_at:
        os.write(fd, PROMPT.encode() + b"\r")
        prompt_sent = True
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
