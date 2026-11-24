#!/usr/bin/env python3
"""End-to-end test with Thonny 4.x/5.x's MicroPython backend (no GUI).

Like `thonny_e2e.py` (Thonny 3.2.7), for the newer backend: its helper is a
Python class with `@classmethod`s that it loads onto the board first.

    pip install pyserial jedi asttokens
    pip download --no-deps thonny==5.0.0 -d /tmp/thonny5
    python3 -m zipfile -e /tmp/thonny5/thonny-5.0.0-py3-none-any.whl /tmp/thonny5/src
    cargo build && python3 tools/thonny5_e2e.py /tmp/thonny5/src [--port /dev/ttyACM0]
"""

import argparse
import os
import pty
import subprocess
import sys
import threading
import tty

HERE = os.path.dirname(os.path.abspath(__file__))
MPY_CLI = os.path.join(HERE, "..", "target", "debug", "mpy-cli")

parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
parser.add_argument("thonny_src", help="directory with the extracted `thonny` package (4.x or 5.x)")
parser.add_argument("--port", help="a real board's serial port instead of mpy-cli")
args = parser.parse_args()
sys.path.insert(0, args.thonny_src)

from thonny.common import InlineCommand, ToplevelCommand  # noqa: E402
from thonny.plugins.micropython import bare_metal_backend as thonny  # noqa: E402
from thonny.plugins.micropython.mp_back import INTERRUPT_CMD  # noqa: E402
from thonny.plugins.micropython.serial_connection import SerialConnection  # noqa: E402

messages = []
_ids = iter(range(1, 1000))


def command(kind, name, **fields):
    """A command as the GUI sends it (with a unique id)."""
    return kind(name, id=next(_ids), **fields)


class Backend(thonny.GenericBareMetalMicroPythonBackend):
    """Thonny's backend without its command loop: the test calls the
    command handlers itself and collects what would go to the GUI."""

    def mainloop(self):
        pass  # connected and prepared: return to the test

    def send_message(self, msg):
        messages.append(msg)


def output(stream="stdout"):
    text = "".join(m.get("data", "") for m in messages if m.get("stream_name") == stream)
    messages.clear()
    return text.replace("\r", "")


failures = 0


def check(what, got, expected):
    global failures
    ok = got == expected
    failures += not ok
    print(("ok  " if ok else "FAIL"), what, "" if ok else f"\n     got:      {got!r}\n     expected: {expected!r}")


if args.port:
    port = args.port
else:
    master, slave = pty.openpty()
    tty.setraw(slave)
    device = subprocess.Popen([MPY_CLI, "--repl"], stdin=master, stdout=master)
    port = os.ttyname(slave)

options = {
    "clean": True,
    "port": port,
    "interrupt_on_connect": True,
    "sync_time": False,
    "validate_time": False,
    "local_rtc": False,
    "submit_mode": "raw",
    "write_block_size": 255,
    "write_block_delay": 0.0,
    "read_block_size": 0,
}
be = Backend(SerialConnection(port, thonny.BAUDRATE), clean=True, args=options)
check("connected and prepared (flat file system)", (be._cwd, be._supports_directories()), ("", False))
messages.clear()

program = "# saved by Thonny 5\nfor i in range(3):\n    print('tick', i)\n"
be._cmd_write_file(command(InlineCommand, "write_file", path="t_thonny.py", content_bytes=program.encode(),
                                 make_shebang_scripts_executable=False))
info = be._cmd_get_dirs_children_info(command(InlineCommand, "get_dirs_children_info", paths=[""], include_hidden=False, node_id=1))
check("file browser lists the file", info["data"][""].get("t_thonny.py", {}).get("size"), len(program))
read = be._cmd_read_file(command(InlineCommand, "read_file", path="t_thonny.py"))
check("read_file content", read["content_bytes"].decode(), program)

blob = bytes(range(256)) * 9
be._cmd_write_file(command(InlineCommand, "write_file", path="t_blob.bin", content_bytes=blob, make_shebang_scripts_executable=False))
check("binary round trip", be._cmd_read_file(command(InlineCommand, "read_file", path="t_blob.bin"))["content_bytes"] == blob, True)
messages.clear()

be._cmd_execute_source(command(ToplevelCommand, "execute_source", source="x = 6 * 7\nprint('x is', x)"))
check("shell statement output", output(), "x is 42\n")
be._cmd_execute_source(command(ToplevelCommand, "execute_source", source="x + 1"))
check("shell expression value", "43" in output(), True)
be._cmd_Run(command(ToplevelCommand, "Run", source=program, args=[], cwd=""))
check("Run output", "tick 0\ntick 1\ntick 2\n" in output(), True)

be._cmd_execute_source(command(ToplevelCommand, "execute_source", source="name = 'TTGO'\nreadings = [1, 2]"))
globs = be._cmd_get_globals(command(InlineCommand, "get_globals", module_name="__main__"))
check("variables view", {"name", "readings"} <= set(globs["globals"]), True)
messages.clear()

be._cmd_execute_source(command(ToplevelCommand, "execute_source", source="1 // 0"))
check("traceback shown", "ZeroDivisionError" in output("stderr"), True)

runner = threading.Thread(target=be._cmd_execute_source, args=(command(ToplevelCommand, "execute_source", source="n = 0\nwhile True:\n    n += 1"),))
runner.start()
thonny.time.sleep(0.5)
be._connection.write(INTERRUPT_CMD)
runner.join(10)
check("Stop interrupts an endless loop", (runner.is_alive(), "KeyboardInterrupt" in output("stderr")), (False, True))

info = be._cmd_get_fs_info(command(InlineCommand, "get_fs_info", path=""))
check("fs info", all(info.get(k) is not None for k in ("total", "used", "free")), True)
be._cmd_delete(command(InlineCommand, "delete", paths=["t_thonny.py", "t_blob.bin"]))
info = be._cmd_get_dirs_children_info(command(InlineCommand, "get_dirs_children_info", paths=[""], include_hidden=False, node_id=2))
check("deleted", sorted(n for n in info["data"][""] if n.startswith("t_")), [])

print("\nall Thonny 4/5 checks passed" if not failures else f"\n{failures} check(s) failed")
if not args.port:
    device.kill()
sys.exit(1 if failures else 0)
