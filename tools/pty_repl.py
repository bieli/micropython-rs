#!/usr/bin/env python3
"""Runs `mpy-cli --repl` on a pseudo-terminal, like a board on a serial port.

Prints the port name; connect any serial tool to it (Thonny, mpremote,
picocom, tools/mpy.py --port ...). The session lasts until Ctrl-C here.

    cargo build && python3 tools/pty_repl.py
    mpremote connect /dev/pts/5 fs ls     # in another terminal
"""

import os
import pty
import subprocess
import sys
import tty

HERE = os.path.dirname(os.path.abspath(__file__))
MPY_CLI = os.path.join(HERE, "..", "target", "debug", "mpy-cli")

master, slave = pty.openpty()
tty.setraw(slave)  # bytes as they are: no echo, no newline translation
device = subprocess.Popen([MPY_CLI, "--repl"], stdin=master, stdout=master)
print(os.ttyname(slave), flush=True)
try:
    device.wait()
except KeyboardInterrupt:
    device.kill()
sys.exit(0)
