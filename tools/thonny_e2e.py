#!/usr/bin/env python3
"""End-to-end test with Thonny's own MicroPython backend (no GUI).

Runs the device-side code of a real Thonny release against micropython-rs:
`mpy-cli --repl` speaks the same REPL protocol as the ESP32 firmware, and a
pseudo-terminal connects it to Thonny's serial connection, so Thonny talks to
it exactly as to a board. Covers connecting, the file browser, opening and
saving files (Thonny's hex-encoded upload/download), running code, the
variables view, deleting, and the storage info.

Usage (Thonny isn't needed as an app, only its sources):

    pip download --no-deps thonny==3.2.7 -d /tmp/thonny  # Thonny 3.2.7 = Ubuntu 20.04's
    cd /tmp/thonny && python3 -m zipfile -e thonny-3.2.7-py3-none-any.whl src
    cargo build && python3 tools/thonny_e2e.py /tmp/thonny/src [--port /dev/ttyACM0]

With `--port`, the same test runs against a real board (it creates and
deletes files named `t_*` there; `main.py` isn't touched).
"""

import argparse
import os
import pty
import subprocess
import sys
import tty

HERE = os.path.dirname(os.path.abspath(__file__))
MPY_CLI = os.path.join(HERE, "..", "target", "debug", "mpy-cli")

parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
parser.add_argument("thonny_src", help="directory with the extracted `thonny` package")
parser.add_argument("--port", help="a real board's serial port instead of mpy-cli")
args = parser.parse_args()
sys.path.insert(0, args.thonny_src)

from thonny.common import InlineCommand, ToplevelCommand  # noqa: E402
from thonny.plugins.micropython import backend as thonny  # noqa: E402
from thonny.plugins.micropython.serial_connection import SerialConnection  # noqa: E402


class Backend(thonny.MicroPythonBackend):
    """Thonny's backend without its command loop: the test calls the
    command handlers itself and collects what would go to the GUI."""

    def __init__(self, connection):  # noqa: D107 (mirrors the original)
        self._connection = connection
        self._local_cwd = None
        self._cwd = None
        self._interrupt_requested = False
        self._cancel_requested = False
        self._command_queue = thonny.Queue()
        self._progress_times = {}
        self._api_stubs_path = os.path.join(args.thonny_src, "thonny", "plugins", "micropython", "api_stubs")
        self._startup_time = thonny.time.time()
        self._ctrl_suggestion_given = False
        self.messages = []
        self._prepare(clean=True)

    def send_message(self, msg):
        self.messages.append(msg)

    def output(self):
        """Program output sent to the shell since the last call."""
        text = "".join(m.get("data", "") for m in self.messages if m.get("event_type") == "ProgramOutput")
        self.messages.clear()
        return text


failures = 0


def check(what, got, expected):
    global failures
    ok = got == expected
    failures += not ok
    print(("ok  " if ok else "FAIL"), what, "" if ok else f"\n     got:      {got!r}\n     expected: {expected!r}")


if args.port:
    port = args.port
else:
    # mpy-cli on one side of a pseudo-terminal, Thonny on the other
    master, slave = pty.openpty()
    tty.setraw(slave)
    device = subprocess.Popen([MPY_CLI, "--repl"], stdin=master, stdout=master)
    port = os.ttyname(slave)

be = Backend(SerialConnection(port, thonny.BAUDRATE))
print("connected; welcome text:", be._welcome_text.splitlines()[0] if be._welcome_text else None)
check("flat file system (no directories)", be._supports_directories(), False)
check("binascii found by help('modules')", "binascii" in be._builtin_modules, True)

# Saving a file from the editor (hex-encoded blocks)
program = "# saved by Thonny\nfor i in range(3):\n    print('tick', i)\n" + "# padding\n" * 40
response = be._cmd_write_file(InlineCommand("write_file", path="t_thonny.py", content_bytes=program.encode(), editor_id=1))
check("write_file response", response["path"], "t_thonny.py")

# The file browser
data = be._cmd_get_dirs_child_data(InlineCommand("get_dirs_child_data", paths={""}, node_id=1))
check("file browser lists the file", data["data"][""].get("t_thonny.py"), {"kind": "file", "size": len(program)})

# Opening it in the editor (hex-encoded download)
read = be._cmd_read_file(InlineCommand("read_file", path="t_thonny.py"))
check("read_file content", (read["error"], read["content_bytes"].decode()), (None, program))

# A binary file larger than one transfer block, with every byte value
blob = bytes(range(256)) * 9
be._cmd_write_file(InlineCommand("write_file", path="t_blob.bin", content_bytes=blob, editor_id=2))
read = be._cmd_read_file(InlineCommand("read_file", path="t_blob.bin"))
check("binary round trip (2304 bytes)", read["content_bytes"] == blob, True)

# Running code from the shell and the editor's Run button
be._cmd_execute_source(ToplevelCommand("execute_source", source="x = 6 * 7\nprint('x is', x)"))
check("shell statement output", be.output().replace("\r", ""), "x is 42\n")
result = be._cmd_execute_source(ToplevelCommand("execute_source", source="x + 1"))
check("shell expression value", result["value_info"].repr, "43")
be._cmd_Run(ToplevelCommand("Run", source=program))
check("Run output", be.output().replace("\r", ""), "tick 0\ntick 1\ntick 2\n")

# The variables view
be._cmd_execute_source(ToplevelCommand("execute_source", source="name = 'TTGO'\nreadings = [1, 2]"))
globs = be._cmd_get_globals(InlineCommand("get_globals", module_name="__main__"))
check("variables view", (globs["globals"].get("name"), globs["globals"].get("readings")), ("'TTGO'", "[1, 2]"))

# An error in the shell
be._cmd_execute_source(ToplevelCommand("execute_source", source="1 // 0"))
err = "".join(m.get("data", "") for m in be.messages if m.get("stream_name") == "stderr")
be.messages.clear()
check("traceback shown", "ZeroDivisionError" in err, True)

# The Stop button: Ctrl-C while a program runs an endless loop
import threading  # noqa: E402

runner = threading.Thread(target=be._cmd_Run, args=(ToplevelCommand("Run", source="n = 0\nwhile True:\n    n += 1"),))
runner.start()
thonny.time.sleep(0.5)
be._connection.write(thonny.INTERRUPT_CMD)
runner.join(10)
err = "".join(m.get("data", "") for m in be.messages if m.get("stream_name") == "stderr")
be.messages.clear()
check("Stop interrupts an endless loop", (runner.is_alive(), "KeyboardInterrupt" in err), (False, True))
result = be._cmd_execute_source(ToplevelCommand("execute_source", source="n > 0"))
check("the board answers after Stop", result["value_info"].repr, "True")

# Stop/Restart backend: a soft reboot clears the variables, files stay
be._cmd_write_file(InlineCommand("write_file", path="t_keep.txt", content_bytes=b"kept", editor_id=3))
be._cmd_execute_source(ToplevelCommand("execute_source", source="t_before_reboot = 1"))
be._soft_reboot(False)  # on a board, main.py runs again here
be.messages.clear()
be._cmd_execute_source(ToplevelCommand("execute_source", source="print('t_before_reboot' in globals())"))
check("soft reboot clears variables", be.output().strip(), "False")
read = be._cmd_read_file(InlineCommand("read_file", path="t_keep.txt"))
check("files survive the soft reboot", read["content_bytes"], b"kept")
be._cmd_delete(InlineCommand("delete", paths=["t_keep.txt"]))

# Storage info, deleting
info = be._cmd_get_fs_info(InlineCommand("get_fs_info", path=""))
check("fs info has total/used/free", sorted(k for k in info if info[k] is not None), ["free", "total", "used"])
be._cmd_delete(InlineCommand("delete", paths=["t_thonny.py", "t_blob.bin"]))
data = be._cmd_get_dirs_child_data(InlineCommand("get_dirs_child_data", paths={""}, node_id=2))
check("deleted", sorted(n for n in data["data"][""] if n.startswith("t_")), [])

print("\nall Thonny checks passed" if not failures else f"\n{failures} check(s) failed")
if not args.port:
    device.kill()
sys.exit(1 if failures else 0)
