#!/usr/bin/env python3
"""Run and save Python scripts on a micropython-rs ESP32 board over USB serial.

Uses MicroPython's raw REPL protocol (the same one `mpremote` uses), so the
firmware doesn't need reflashing to try or replace a script.

    tools/mpy.py run scripts/blink.py      # run a file, print its output
    tools/mpy.py exec "print(6 * 7)"       # run a snippet
    tools/mpy.py save scripts/blink.py     # store as main.py in flash (runs at boot)
    tools/mpy.py cat                       # print the saved main.py
    tools/mpy.py erase                     # remove main.py (the built-in script runs again)
    tools/mpy.py ls                        # list the files on the board
    tools/mpy.py put lib/tools.py          # copy a file to the board (as tools.py)
    tools/mpy.py put local.txt remote.txt  # ... under another name
    tools/mpy.py get remote.txt [local]    # copy a file from the board
    tools/mpy.py rm remote.txt             # delete a file on the board
    tools/mpy.py reset                     # soft reset: clears variables, reruns main.py
    tools/mpy.py watch scripts/voltmeter.py         # rerun the file on the board at every save
    tools/mpy.py watch scripts/voltmeter.py --save  # ... and store each version as main.py

Options: --port /dev/ttyUSB0 (default: /dev/ttyACM0 or $MPY_PORT), --baud 115200.

Before each command it asks the board which commit its firmware is built from
(`#@version`) and warns when that's older than this checkout (changes that
don't reach the firmware, like docs and tests, don't count), with the
`make flash` command that brings it up to date; --no-check skips that.
Needs pyserial (`pip install pyserial`). Exit code 1 if the script raised an error.
"""

import argparse
import codecs
import re
import os
import subprocess
import sys
import time

try:
    import serial
except ImportError:
    sys.exit("tools/mpy.py needs pyserial: pip install pyserial")

RAW_PROMPT = b"raw REPL; CTRL-B to exit\r\n>"
# Data bytes per `#@write` (sent as twice as many hex characters)
PUT_CHUNK = 2048


class FileCommandError(Exception):
    pass


# ---- is the board's firmware up to date? ------------------------------------

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# Changes here don't reach the firmware: a commit that touched only these
# doesn't make the board's firmware out of date
NOT_FIRMWARE = ("tests/", "examples/", "fuzz/", ".github/", "docs/",
                "micropython-esp32/tools/", "micropython-esp32/scripts/")


def git(*args):
    """git's output in this checkout ("" for success without output), or
    None when git fails or isn't there."""
    try:
        out = subprocess.run(["git", "-C", REPO, *args], capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return out.stdout.strip() if out.returncode == 0 else None


def board_version(board):
    """{'version', 'commit', 'chip', 'level', 'features', 'float', 'mhz'}
    from the firmware, or None when it doesn't answer `#@version` (older
    firmware runs it as a Python comment, or rejects it)."""
    output, error = board.execute("#@version", stream=False)
    text = output.decode("utf-8", "replace")
    if error or "commit=" not in text:
        return None
    return dict(item.split("=", 1) for item in text.split() if "=" in item)


def flash_command(info):
    """The `make flash` that builds firmware like the board's (from
    micropython-esp32/)."""
    if info is None:
        return "make flash [CHIP=esp32s3|esp32c3] [FEATURES=tft,wifi]  (the options it was built with)"
    parts = ["make flash"]
    if info.get("chip", "esp32") != "esp32":
        parts.append("CHIP=" + info["chip"])
    if info.get("level", "full") != "full":
        parts.append("LEVEL=" + info["level"])
    if info.get("features"):
        parts.append("FEATURES=" + info["features"])
    if info.get("float") == "32":
        parts.append("FLOAT=32")
    if info.get("mhz", "80") != "80":
        parts.append("CPU_MHZ=" + info["mhz"])
    return " ".join(parts)


def firmware_warning(info):
    """Why the board's firmware may not match this checkout, or None."""
    if info is None:
        return ("the board's firmware is older than this checkout: it doesn't report its version, so it\n"
                "predates that and may lack what scripts here use (errors like `UnexpectedToken` or\n"
                "`ImportError` can come from that)")
    commit = info.get("commit", "unknown")
    base = commit[: -len("-dirty")] if commit.endswith("-dirty") else commit
    if commit == "unknown" or git("rev-parse", "--short=7", "HEAD") is None:
        return None  # built outside a checkout, or no git here: nothing to compare
    if git("cat-file", "-e", base + "^{commit}") is None:
        return f"the board's firmware is from commit {commit}, which isn't in this checkout"
    if git("merge-base", "--is-ancestor", base, "HEAD") is None:
        return f"the board's firmware is from commit {commit}, which isn't part of this checkout's history (another branch?)"
    changed = (git("diff", "--name-only", base, "HEAD") or "").splitlines()
    relevant = [f for f in changed if not f.endswith(".md") and not f.startswith(NOT_FIRMWARE)]
    if not relevant:
        return None
    count = git("rev-list", "--count", base + "..HEAD") or "some"
    head = git("rev-parse", "--short=7", "HEAD")
    return (f"the board's firmware is from commit {commit}, {count} commit(s) older than this\n"
            f"checkout ({head}), with changes to the firmware since (e.g. {relevant[0]})")


def check_firmware(board):
    """Warns (on stderr) when the board's firmware is out of date; never fails."""
    try:
        info = board_version(board)
        warning = firmware_warning(info)
    except Exception:  # an odd answer must not stop the command itself
        return
    if warning:
        sys.stderr.write(f"mpy.py: warning: {warning}.\n"
                         f"  Flash the current firmware, from micropython-esp32/:\n"
                         f"    {flash_command(info)}\n"
                         f"  (--no-check hides this)\n")
        sys.stderr.flush()


class Board:
    def __init__(self, port, baud):
        self.serial = serial.Serial(port, baud, timeout=0.1)

    def read_until(self, ending, timeout=10.0, echo=None):
        """Reads until `ending`; `echo` gets each chunk as it arrives."""
        data = b""
        deadline = time.time() + timeout
        while not data.endswith(ending):
            chunk = self.serial.read(1)
            if chunk:
                data += chunk
                if echo:
                    echo(chunk)
                deadline = time.time() + timeout
            elif time.time() > deadline:
                raise TimeoutError(f"no {ending!r} from the board, got {data[-80:]!r}")
        return data

    def enter_raw(self):
        # Ctrl-C twice stops a running script, Ctrl-A enters the raw REPL
        self.serial.write(b"\r\x03\x03")
        time.sleep(0.2)
        self.serial.reset_input_buffer()
        self.serial.write(b"\r\x01")
        self.read_until(RAW_PROMPT)

    def exit_raw(self):
        self.serial.write(b"\r\x02")

    def soft_reset(self):
        self.serial.write(b"\x04")
        self.read_until(b"soft reboot\r\n")
        self.read_until(RAW_PROMPT)

    def send_code(self, code):
        """Sends `code` to the raw REPL and starts it (Ctrl-D, then `OK`)."""
        data = code.encode() if isinstance(code, str) else code
        for i in range(0, len(data), 256):
            self.serial.write(data[i : i + 256])
            time.sleep(0.01)
        self.serial.write(b"\x04")
        reply = self.serial.read(2)
        if reply != b"OK":
            raise RuntimeError(f"board did not accept the code: {reply!r}")

    def command(self, code):
        """Runs a `#@` file command; returns its output, raises on an error."""
        output, error = self.execute(code, stream=False)
        if error:
            raise FileCommandError(error_summary(error.decode("utf-8", "replace")))
        return output

    def put(self, data, name):
        """Uploads bytes as file `name` (hex-encoded in chunks, so any byte is safe)."""
        self.command("#@open " + name)
        for i in range(0, len(data), PUT_CHUNK):
            self.command("#@write " + data[i : i + PUT_CHUNK].hex())
        written = int(self.command("#@close").decode() or "-1")
        if written != len(data):
            raise FileCommandError(f"board stored {written} bytes, expected {len(data)}")

    def get(self, name):
        return bytes.fromhex(self.command("#@get " + name).decode())

    def execute(self, code, stream=True):
        """Runs `code` in the raw REPL. Returns (output, error) bytes."""
        self.send_code(code)

        # Output arrives byte by byte: decode incrementally, so multi-byte
        # UTF-8 characters (e.g. "°", "ł", "☺") aren't split
        decoder = codecs.getincrementaldecoder("utf-8")("replace")

        def show(chunk):
            if stream and chunk != b"\x04":
                sys.stdout.write(decoder.decode(chunk))
                sys.stdout.flush()

        output = self.read_until(b"\x04", timeout=3600, echo=show)[:-1]
        error = self.read_until(b"\x04")[:-1]
        self.read_until(b">")
        return output, error


def error_summary(text):
    """The last line of a traceback, with its line number:
    `ZeroDivisionError: division by zero (line 2)`."""
    lines = [line.strip() for line in text.strip().splitlines() if line.strip()]
    if not lines:
        return ""
    where = re.search(r"line (\d+)", text)
    if where and len(lines) > 1:
        return f"{lines[-1]} (line {where.group(1)})"
    return lines[-1]


class Watcher:
    """`watch`: runs a file on the board and restarts it whenever it's saved.

    The board's output is shown live. A running program, even an endless
    loop, is stopped with Ctrl-C before the new version starts.
    """

    def __init__(self, board, path, save):
        self.board = board
        self.path = path
        self.save = save
        self.running = False
        self.section = 0  # 0 = program output, 1 = error text, 2 = waiting for ">"
        self.error = b""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def run_forever(self):
        last_mtime = None
        print(f"watching {self.path} - save it to rerun it on the board, Ctrl-C to stop")
        while True:
            try:
                mtime = os.stat(self.path).st_mtime
            except FileNotFoundError:
                mtime = None
            if mtime is not None and mtime != last_mtime:
                last_mtime = mtime
                time.sleep(0.1)  # let the editor finish writing
                self.restart()
            self.pump(0.2)

    def restart(self):
        with open(self.path, "rb") as f:
            code = f.read()
        # The board runs a subset of Python, so CPython finds syntax errors first
        try:
            compile(code, self.path, "exec")
        except SyntaxError as err:
            print(f"\n{self.path}:{err.lineno}: syntax error: {err.msg} - not uploaded")
            return
        if self.running:
            self.stop()
        self.board.soft_reset()
        if self.save:
            _, error = self.board.execute(b"#@save\n" + code, stream=False)
            if error:
                print(error.decode("utf-8", "replace").strip())
        stamp = time.strftime("%H:%M:%S")
        saved = " and saved as main.py" if self.save else ""
        print(f"\n--- {stamp} running {self.path}{saved} ---")
        self.board.send_code(code)
        self.running = True
        self.section = 0
        self.error = b""

    def stop(self):
        """Interrupts the running program and waits for the raw prompt."""
        self.board.serial.write(b"\x03")
        deadline = time.time() + 5
        while self.running and time.time() < deadline:
            self.pump(0.1)
        if self.running:
            raise TimeoutError("the program didn't stop after Ctrl-C")

    def pump(self, seconds):
        """Shows the board's output for up to `seconds`."""
        deadline = time.time() + seconds
        while time.time() < deadline:
            data = self.board.serial.read(256)
            if not data:
                continue
            if not self.running:
                continue
            for byte in data:
                self.feed(byte)

    def feed(self, byte):
        # Raw REPL reply: output, \x04, error text, \x04, ">"
        if self.section == 0:
            if byte == 4:
                self.section = 1
            else:
                sys.stdout.write(self.decoder.decode(bytes([byte])))
                sys.stdout.flush()
        elif self.section == 1:
            if byte == 4:
                self.section = 2
            else:
                self.error += bytes([byte])
        elif byte == ord(">"):
            self.running = False
            error = error_summary(self.error.decode("utf-8", "replace"))
            if error.startswith("KeyboardInterrupt"):
                print("--- stopped ---")
            elif error:
                print(f"--- {error} ---")
            else:
                print("--- program finished ---")

    def finish(self):
        """Ctrl-C on the host: stop the program; with --save, let main.py run."""
        if self.running:
            self.stop()
        if self.save:
            self.board.soft_reset()
            self.board.exit_raw()
            time.sleep(0.3)
            self.board.serial.write(b"\x04")  # soft reset in the normal REPL runs main.py
            print("\nboard runs the saved main.py on its own now")
        else:
            print("\nstopped watching")


def file_command(board, args):
    if args.command == "ls":
        sys.stdout.write(board.command("#@ls").decode("utf-8", "replace").replace("\r\n", "\n"))
    elif args.command in ("put", "save"):
        with open(args.arg, "rb") as f:
            data = f.read()
        name = "main.py" if args.command == "save" else (args.arg2 or os.path.basename(args.arg))
        board.put(data, name)
        print(f"saved {name} ({len(data)} bytes)")
    elif args.command == "get":
        data = board.get(args.arg)
        local = args.arg2 or os.path.basename(args.arg)
        with open(local, "wb") as f:
            f.write(data)
        print(f"{args.arg} -> {local} ({len(data)} bytes)")
    elif args.command == "rm":
        board.command("#@rm " + args.arg)
        print(f"removed {args.arg}")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", default=os.environ.get("MPY_PORT", "/dev/ttyACM0"))
    parser.add_argument("--baud", type=int, default=115200)
    parser.add_argument(
        "command", choices=["run", "exec", "save", "cat", "erase", "reset", "watch", "ls", "put", "get", "rm"]
    )
    parser.add_argument("arg", nargs="?", help="file (run/save/watch/put/get/rm) or code (exec)")
    parser.add_argument("arg2", nargs="?", help="put/get: name on the other side")
    parser.add_argument("--save", action="store_true", help="watch: also store each version as main.py")
    parser.add_argument("--no-check", action="store_true", help="don't compare the board's firmware with this checkout")
    args = parser.parse_args()

    if args.command in ("run", "save", "exec", "watch", "put", "get", "rm") and not args.arg:
        parser.error(f"{args.command} needs an argument")

    try:
        board = Board(args.port, args.baud)
    except serial.SerialException as err:
        sys.exit(f"Cannot open {args.port}: {err}\nPick the board's port with --port (e.g. /dev/ttyUSB0) or $MPY_PORT.")
    try:
        board.enter_raw()
    except TimeoutError:
        sys.exit(
            f"The board on {args.port} doesn't answer the raw REPL.\n"
            "Most likely it runs firmware older than the REPL (or other firmware, e.g. MicroPython\n"
            "with a different setup). Flash the current firmware once, then retry:\n"
            "    make flash FEATURES=tft      # TTGO T-Display (plain ESP32 boards: make flash)\n"
            "If more than one board is connected, pick the right one with --port."
        )
    if not args.no_check:
        check_firmware(board)
    if args.command == "watch":
        watcher = Watcher(board, args.arg, args.save)
        try:
            watcher.run_forever()
        except KeyboardInterrupt:
            watcher.finish()
            if not args.save:
                board.exit_raw()
        return 0

    try:
        if args.command in ("ls", "put", "get", "rm", "save"):
            try:
                return file_command(board, args)
            except FileCommandError as err:
                sys.exit(f"Error: {err}")
        if args.command == "reset":
            board.soft_reset()
            print("soft reset done")
            return 0

        if args.command == "run":
            with open(args.arg, "rb") as f:
                code = f.read()
        elif args.command == "exec":
            code = args.arg.encode()
        elif args.command == "cat":
            code = b"#@cat"
        else:
            code = b"#@erase"

        # Start from a clean interpreter for scripts, like `mpremote run`
        if args.command in ("run", "exec"):
            board.soft_reset()
        _, error = board.execute(code)
        if error:
            sys.stderr.write(error.decode("utf-8", "replace"))
            return 1
        return 0
    finally:
        board.exit_raw()


if __name__ == "__main__":
    try:
        sys.exit(main())
    except BrokenPipeError:
        # Output piped into e.g. `head`, which closed early: not an error
        sys.stderr.close()
        sys.exit(0)
