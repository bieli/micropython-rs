#!/usr/bin/env python3
"""Run and save Python scripts on a micropython-rs ESP32 board over USB serial.

Uses MicroPython's raw REPL protocol (the same one `mpremote` uses), so the
firmware doesn't need reflashing to try or replace a script.

    tools/mpy.py run scripts/blink.py      # run a file, print its output
    tools/mpy.py exec "print(6 * 7)"       # run a snippet
    tools/mpy.py save scripts/blink.py     # store as main.py in flash (runs at boot)
    tools/mpy.py cat                       # print the saved main.py
    tools/mpy.py erase                     # remove main.py (the built-in script runs again)
    tools/mpy.py reset                     # soft reset: clears variables, reruns main.py

Options: --port /dev/ttyUSB0 (default: /dev/ttyACM0 or $MPY_PORT), --baud 115200.
Needs pyserial (`pip install pyserial`). Exit code 1 if the script raised an error.
"""

import argparse
import os
import sys
import time

try:
    import serial
except ImportError:
    sys.exit("tools/mpy.py needs pyserial: pip install pyserial")

RAW_PROMPT = b"raw REPL; CTRL-B to exit\r\n>"


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

    def execute(self, code, stream=True):
        """Runs `code` in the raw REPL. Returns (output, error) bytes."""
        data = code.encode() if isinstance(code, str) else code
        for i in range(0, len(data), 256):
            self.serial.write(data[i : i + 256])
            time.sleep(0.01)
        self.serial.write(b"\x04")
        reply = self.serial.read(2)
        if reply != b"OK":
            raise RuntimeError(f"board did not accept the code: {reply!r}")

        def show(chunk):
            if stream and chunk != b"\x04":
                sys.stdout.write(chunk.decode("utf-8", "replace"))
                sys.stdout.flush()

        output = self.read_until(b"\x04", timeout=3600, echo=show)[:-1]
        error = self.read_until(b"\x04")[:-1]
        self.read_until(b">")
        return output, error


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", default=os.environ.get("MPY_PORT", "/dev/ttyACM0"))
    parser.add_argument("--baud", type=int, default=115200)
    parser.add_argument("command", choices=["run", "exec", "save", "cat", "erase", "reset"])
    parser.add_argument("arg", nargs="?", help="file (run/save) or code (exec)")
    args = parser.parse_args()

    if args.command in ("run", "save", "exec") and not args.arg:
        parser.error(f"{args.command} needs an argument")

    board = Board(args.port, args.baud)
    board.enter_raw()
    try:
        if args.command == "reset":
            board.soft_reset()
            print("soft reset done")
            return 0

        if args.command == "run":
            with open(args.arg, "rb") as f:
                code = f.read()
        elif args.command == "exec":
            code = args.arg.encode()
        elif args.command == "save":
            with open(args.arg, "rb") as f:
                code = b"#@save\n" + f.read()
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
    sys.exit(main())
