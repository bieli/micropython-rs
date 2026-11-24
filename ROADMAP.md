# micropython-rs Roadmap

What comes next for micropython-rs, ordered by what embedded engineers need first.
Status: ✅ done · 🚧 in progress · ⬜ planned · ➡️ moved

## Phase 1: Code upgrades (foundation)

| # | Item | Why it matters | Done when | Status |
|---|------|----------------|-----------|--------|
| 1.1 | **Calls inside expressions**: `x = adc_read(34) * 3`, `print(millis())` | `HardwareBridge::call` already returns `Option<i64>`, but the result was dropped. Every sensor feature needs this | Calls are an RPN item, and a `None` result used in math is a `TypeError` | ✅ |
| 1.2 | **Error line numbers**: `Error at line 7: ZeroDivision` | On a device there's no debugger, so a bare `Err(ZeroDivision)` is hard to trace | The lexer tracks lines, and `Interpreter::error_line()` reports where the last error happened | ✅ |
| 1.3 | **Fuzzing the parser and interpreter** (`cargo-fuzz`) | Scripts are untrusted input, and a panic means the firmware resets | A fuzz target runs in CI for N minutes without panics or stack overflows | ⬜ |
| 1.4 | **Execution step limit and abort flag** | Once loops exist, `while 1:` freezes the chip with no way out | `set_step_limit(n)` plus an `AtomicBool` abort flag (e.g. set by a UART Ctrl-C handler) | ✅ |
| 1.5 | **CI for firmware**: build both ESP32 variants in the `espressif/idf-rust` container and track binary size | Firmware breakage is currently caught only by hand | The CI job fails on build errors and reports `.text` size | ⬜ |
| 1.6 | **`#![forbid(unsafe_code)]` and stack budget docs** | Embedded users need to know the RAM cost up front | The README lists worst-case stack per `eval()`, measured on the ESP32 | ⬜ |

## Phase 2: Language features embedded engineers need most

| # | Feature | Why | Status |
|---|---------|-----|--------|
| 2.1 | **`if`/`elif`/`else`, `while`, `break`/`continue`**, with indentation | Without loops, no real firmware logic is possible. This is the biggest single unlock | ✅ |
| 2.2 | **Comparisons and booleans**: `== != < <= > >=`, `and or not`, `True`/`False` | Needed by 2.1 | ✅ |
| 2.3 | **Hex and binary literals and bitwise operators**: `0x40`, `0b1010`, `& \| ^ ~ << >>` | Daily work with registers, flags and masks | ✅ |
| 2.4 | **Augmented assignment**: `+= -= \|= &= <<=` | Common in counters and bit manipulation | ✅ |
| 2.5 | **`def` functions** (no closures) | Reusable code such as `blink(n)`. Needs a fixed-size call frame stack | ✅ |
| 2.6 | **`bytes`/`bytearray` from a static buffer pool** | Needed for I²C, SPI and UART payloads, while staying zero-heap. Moved to Phase 3: it depends on the bus API design (`HardwareBridge` passes only integers today) | ➡️ 3.4 |

Phase 2 follow-ups found while building it:

* `for` loops with `range()`, the most common loop form in MicroPython code.
* ✅ `global` statement, so functions can update global counters and state (done, with `abs`/`min`/`max`).
* `hex()`/`bin()` output for register values (needs a small string pool, like 2.6).

## Phase 3: Hardware API (MicroPython-style names)

Start with flat functions that follow MicroPython's names. Move to `machine.Pin(...)`-style objects once attribute access exists.

| # | API | Notes | Status |
|---|-----|-------|--------|
| 3.1 | `sleep_ms(ms)`, `sleep_us(us)`, `ticks_ms()`, `ticks_diff(a, b)` | Same names as MicroPython's `time` module. Keep `delay_ms` as an alias | ✅ |
| 3.2 | `pin_mode(pin, OUT/IN/IN_PULLUP)`, `pin_write(pin, v)`, `pin_read(pin)` | Runtime pin numbers with an allowlist per board, so scripts can't touch the flash or PSRAM pins | ✅ |
| 3.3 | `adc_read(pin)`, `pwm(pin, freq, duty)` | Sensors, LED dimming and servos | ✅ |
| 3.4 | `i2c_write(addr, data)`, `i2c_read(addr, n)`, `i2c_scan()` | `i2c_scan()` is the first thing everyone runs with a new sensor. Needs 2.6 | 🚧 |
| 3.5 | `uart_write`/`uart_read` and SPI transfers | GPS modules, modems, displays | ⬜ |
| 3.6 | Watchdog: `wdt_feed()`, plus automatic feeding inside `sleep_ms` | Production firmware has to recover from a hung script | ✅ |
| 3.7 | TFT functions: `cls()`, `color(r, g, b)`, `text(x, y, s)`, `backlight(on)`, and scrolling | Makes the `tft` feature useful beyond showing logs | ✅ |
| 3.8 | **Native modules with `import`**: `import display`, `from time import sleep_ms`; a `display` drawing module (pixels, lines, rectangles, circles, triangles, text) and a `time` module | Rust code grouped into MicroPython-style modules instead of one flat namespace | ✅ |
| 3.9 | **`buttons` module and apps**: debounced short/long presses, `display.number()` and fonts, and a two-button calculator (`scripts/calculator.py`) | Real interactive apps on the board without a PC | ✅ |
| 3.10 | **Calibrated ADC** (`adc_mv`, eFuse Vref), `battery_mv()`, and a voltmeter app (`scripts/voltmeter.py`) | Readings in millivolts instead of raw counts. ESP-IDF's 11 dB lookup table is still missing | ✅ |

Design note (✅ done): `HardwareBridge` now has a function table (`functions() -> &[HwFn]`) with argument counts,
used for checks and `help()`, plus `constants()` and string arguments (`call_args`). Dispatch is still a `match` on the name,
which keeps the trait simple.

Phase 3 status:

* 3.4 I²C: `i2c_probe`, `i2c_scan`, and single-byte / 16-bit register reads and writes are done. Multi-byte transfers
  still need `bytes`/`bytearray` (2.6).
* 3.5 UART/SPI: not started. It needs the same byte buffers.
* PWM output and the I²C transfers were checked for errors on a TTGO T-Display, but not measured with a scope or a real sensor.

## Phase 4: Developer experience on the device

| # | Feature | Why | Status |
|---|---------|-----|--------|
| 4.1 | **REPL over USB/UART** with line editing and Ctrl-C | The defining MicroPython experience: try code without reflashing | ✅ |
| 4.2 | **Script from flash**: `include_str!` at build time (`make flash SCRIPT=blink.py`), then a data partition with `main.py` | Users change Python code without touching Rust. Done: build-time embedding, and `main.py` saved in the `nvs` partition over USB (`tools/mpy.py save`) | ✅ |
| 4.3 | **Upload a script over serial** (MicroPython's raw REPL: works with `mpremote run/exec` and `tools/mpy.py`) | Fast edit, run, edit cycle | ✅ |
| 4.4 | **Example gallery** in `micropython-esp32/scripts/`: blink, button, ADC, I²C scan, TFT clock | Each one also doubles as a hardware smoke test | ✅ |
| 4.5 | **More boards**: ESP32-C3 and C6 (RISC-V), and an SSD1306 OLED feature | Cheap boards that many people already own | ⬜ Not started: needs the hardware to test on |

Phase 4 follow-ups found while building it:

* `str()`, string concatenation and formatting (`text()` can't show a number yet), which need a small string pool.
* Importing `.py` files as modules (only native Rust modules can be imported), and a `machine` module with `Pin` objects.
* 🚧 A file system: done as a flat file system in a `vfs` partition (`src/flatfs.rs`, `tools/mpy.py ls/put/get/rm`). Still open: `open()` / `os` for scripts, so `mpremote cp`/`ls` and Thonny's file browser work.
* Raw-paste mode for faster uploads (plain raw mode works).

## Phase 5: Python features for Thonny (and for scripts)

Thonny reads and saves device files by running Python helper code on the board (see README, "Files on the board").

| # | Feature | Needed by | Status |
|---|---------|-----------|--------|
| 5.1 | **Real strings** on a fixed-size heap with a garbage collector: escapes, `+`/`*`, `len`, indexing/slicing, `in`, `str()`/`repr()`, `int()`/`ord()`/`chr()`, string methods | Thonny 3.2.7+ | ✅ |
| 5.2 | **`bytes`**: `b'...'` literals, `binascii.hexlify`/`unhexlify`, `encode()`/`decode()`, `hex()` | Thonny 3.2.7+ (file transfer) | ✅ |
| 5.3 | **Containers**: list, tuple, dict, `for` with `range()`, list/dict comprehensions, `split`/`join`, `del`, `is`, `x if c else y` | Thonny 3.2.7+ | ✅ |
| 5.4 | **Exceptions**: `try`/`except`/`else`/`finally`, `OSError`/`ImportError`, `except X as e`, `raise`, `assert`, Python tracebacks | Thonny 3.2.7+ | ✅ |
| 5.5 | **Built-ins**: keyword arguments (`print(..., sep=, end=)`), default parameters, `lambda`, functions as values, `hasattr`/`getattr`, `globals()`, `id`, `isinstance`/`type`, `dir`, `builtins`, `%`/`format()`/f-strings | Thonny 3.2.7+ | ✅ |
| 5.6 | **`os` and `open()`** on the flash file system: file objects (read/readline/write/with/iteration), `os.listdir/remove/rename/stat/size/statvfs/uname` | Thonny 3.2.7+, `mpremote cp/ls` | ✅ |
| 5.7 | **Thonny 3.2.7 end to end**: connect, file list, open `main.py`, save, Run/Stop (`tools/thonny_e2e.py` with Thonny's own backend code; also `mpremote fs`; bound methods, `os.ilistdir`) | milestone | ✅ on the PC (`mpy-cli --repl`); on the board: run `tools/thonny_e2e.py --port` |
| 5.8 | **Classes** (inheritance, `super()`, `@classmethod`/`@staticmethod`, `__str__`/`__enter__`, own exceptions), closures and decorators, `*args`/`**kwargs`, `sys`, `os.getcwd` | Thonny 4.x/5.x | ✅ (`tools/thonny5_e2e.py`: Thonny 4.1.7 and 5.0.0) |

**Phase 5 is done**, on the PC and on a TTGO T-Display:

* Thonny 3.2.7, 4.1.7 and 5.0.0 and mpremote work: `tools/thonny_e2e.py` and `tools/thonny5_e2e.py` pass against
  `mpy-cli --repl` (in CI) and against the board (`--port /dev/ttyACM0`).
* Every language feature is checked against CPython: `tests/cpython/*.py` must print what CPython printed, run by
  `cargo test` and, with `tools/mpy.py run`, on the board (all identical).
* Found on the board: the stack. The REPL's buffers were built on the stack before being copied to their statics
  (64 KB of the 118 KB stack, now `ConstStaticCell`), the old-`main.py` migration buffer stayed in `main`'s frame
  (24 KB, now out of line), and deep recursion overflowed it. The interpreter now has a stack guard
  (`set_stack_floor`): too deep raises `RecursionError` instead of crashing the board.

Left for later (documented in the README): floats, sets, generators, `nonlocal`, operator methods (`__add__`,
`__eq__`, ...), multiple inheritance, importing `.py` files, `bytearray`, step slices (`a[::2]`).

Found while building 5.1:

* **Speed on the ESP32:** with the heap and the shared value stack, tight loops got ~20% slower on the ESP32
  (5000 empty `while` passes: 829 ms before, 1005 ms after) although the host was unchanged, and after 5.8 they
  took 1635 ms: the flash instruction cache. Fixed with the `iram` feature of the core library (the hot loop runs
  from IRAM): 897 ms, with all of Phase 5 in; drawing 50 pixels takes 31 ms (34 ms before Phase 5).

**Later:** once loops and functions exist and profiling shows the evaluator is a bottleneck, add the
bytecode compiler and VM, plus a `const` folding pass.

## Recommended order

1. **1.1, 1.2, 1.4**: calls in expressions, line numbers, step limit. Small, and later work depends on them.
2. **2.1–2.3**: control flow, comparisons, bitwise operators. After this, real firmware scripts become possible.
3. **3.1–3.2 and 4.2**: time, GPIO, script from a file. A usable product: a `main.py` that blinks, reads buttons and loops.
4. **4.1**: REPL. Then I²C and ADC (3.3–3.4), with fuzzing and firmware CI (1.3, 1.5) in parallel.
