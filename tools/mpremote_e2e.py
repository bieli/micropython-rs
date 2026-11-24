#!/usr/bin/env python3
"""End-to-end test with mpremote (MicroPython's official tool) against
`mpy-cli --repl` on a pseudo-terminal: copying files both ways (text and
binary), ls, cat, rm, exec, eval and run.

    pip install mpremote && cargo build && python3 tools/mpremote_e2e.py
"""

import os
import pty
import subprocess
import sys
import tempfile
import tty

HERE = os.path.dirname(os.path.abspath(__file__))
MPY_CLI = os.path.join(HERE, "..", "target", "debug", "mpy-cli")
MPREMOTE = [sys.executable, "-m", "mpremote"]

master, slave = pty.openpty()
tty.setraw(slave)
device = subprocess.Popen([MPY_CLI, "--repl"], stdin=master, stdout=master)
port = os.ttyname(slave)
failures = 0


def mpremote(*args):
    result = subprocess.run(MPREMOTE + ["connect", port, *args], capture_output=True, text=True, timeout=60)
    return (result.stdout + result.stderr).replace("\r", "")


def check(what, got, expected):
    global failures
    ok = expected in got if isinstance(expected, str) else got == expected
    failures += not ok
    print(("ok  " if ok else "FAIL"), what, "" if ok else f"\n     got:      {got!r}\n     expected: {expected!r}")


with tempfile.TemporaryDirectory() as tmp:
    script = os.path.join(tmp, "hello.py")
    with open(script, "w") as f:
        f.write("print('hello from a file')\n")
    blob = os.path.join(tmp, "blob.bin")
    with open(blob, "wb") as f:
        f.write(bytes(range(256)) * 12)

    mpremote("fs", "cp", script, ":hello.py")
    mpremote("fs", "cp", blob, ":blob.bin")
    check("fs ls", mpremote("fs", "ls"), "        3072 blob.bin")
    check("fs cat", mpremote("fs", "cat", "hello.py"), "print('hello from a file')")
    back = os.path.join(tmp, "back.bin")
    mpremote("fs", "cp", ":blob.bin", back)
    with open(back, "rb") as f:
        check("binary copy back", f.read() == bytes(range(256)) * 12, True)
    check("exec", mpremote("exec", "import os; print(sorted(os.listdir()))"), "['blob.bin', 'hello.py']")
    check("eval", mpremote("eval", "6 * 7"), "42")
    check("run", mpremote("run", script), "hello from a file")
    check("missing file", mpremote("fs", "cat", "nope.txt"), "No such file or directory")
    mpremote("fs", "rm", "hello.py")
    mpremote("fs", "rm", "blob.bin")
    check("fs rm", mpremote("exec", "import os; print(os.listdir())"), "[]")

device.kill()
print("\nall mpremote checks passed" if not failures else f"\n{failures} check(s) failed")
sys.exit(1 if failures else 0)
