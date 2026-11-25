# micropython-rs Roadmap

What comes next for micropython-rs, ordered by what embedded engineers need first.
Status: ✅ done · 🚧 in progress · ⬜ planned · ➡️ moved

## Phase 1: Code upgrades (foundation)

| # | Item | Why it matters | Done when | Status |
|---|------|----------------|-----------|--------|
| 1.1 | **Calls inside expressions**: `x = adc_read(34) * 3`, `print(millis())` | `HardwareBridge::call` already returns `Option<i64>`, but the result was dropped. Every sensor feature needs this | Calls are an RPN item, and a `None` result used in math is a `TypeError` | ✅ |
| 1.2 | **Error line numbers**: `Error at line 7: ZeroDivision` | On a device there's no debugger, so a bare `Err(ZeroDivision)` is hard to trace | The lexer tracks lines, and `Interpreter::error_line()` reports where the last error happened | ✅ |
| 1.3 | **Fuzzing the parser and interpreter** (`cargo-fuzz`) | Scripts are untrusted input, and a panic means the firmware resets | A fuzz target runs in CI for N minutes without panics or stack overflows | ✅ `fuzz/fuzz_targets/eval.rs`, 5 minutes in CI |
| 1.4 | **Execution step limit and abort flag** | Once loops exist, `while 1:` freezes the chip with no way out | `set_step_limit(n)` plus an `AtomicBool` abort flag (e.g. set by a UART Ctrl-C handler) | ✅ |
| 1.5 | **CI for firmware**: build both ESP32 variants in the `espressif/idf-rust` container and track binary size | Firmware breakage is currently caught only by hand | The CI job fails on build errors and reports `.text` size | ✅ both builds; `tools/fw_report.py` also checks the stack |
| 1.6 | **`#![forbid(unsafe_code)]` and stack budget docs** | Embedded users need to know the RAM cost up front | The README lists worst-case stack per `eval()`, measured on the ESP32 | ✅ |

Robustness work on 1.3, 1.5 and 1.6 (done after Phase 6):

* **1.3 Fuzzing:** `cargo +nightly fuzz run eval` sends random scripts through the lexer, parser, evaluator, heap and
  garbage collector, files and `import`, as several `eval()` calls on one interpreter (like a REPL session), with a
  small heap so out-of-memory paths run often and no stack floor so unbounded recursion would crash. Seeded with the
  conformance and example scripts and a dictionary of Python keywords. 4.1 million inputs (24 minutes, 10 workers)
  found nothing: no panic, overflow, hang or stack overflow. CI fuzzes for 5 minutes on every push.
* **1.5 Firmware CI:** both ESP32 builds in Espressif's container (as `make build`), then `tools/fw_report.py`: flash,
  IRAM, DRAM and `dram2` use, the main stack, and the biggest stack frames, decoded from each function's Xtensa
  `entry` instruction (and the `l32r`/`movsp` sequence of frames over 32 KB). It fails when the main stack drops
  below 128 KB or one function takes more than 32 KB of it: the Phase 5 stack bugs (an 80 KB frame, 64 KB of the
  stack lost to buffers) would have failed it. Checked by running the job's commands on a clean copy of the repository.
* **1.6 Stack budget:** measured on the TTGO with stack painting (`stack_peak()`, `scripts/stack_budget.py`), now in
  the README: 35 KB for a statement at the top level, 7.8 KB per call, 1.6 KB per nested block, 147 KB in the worst
  case found, of a 181 KB stack. Found while measuring: the REPL kept a second, temporary copy of the 15 KB
  interpreter in its stack frame (moves of the `with_` builders); built out of line now, which gave every script
  17 KB more stack. The core library is `#![forbid(unsafe_code)]` (the `iram` build only allows its `link_section`).

## Phase 2: Language features embedded engineers need most

| # | Feature | Why | Status |
|---|---------|-----|--------|
| 2.1 | **`if`/`elif`/`else`, `while`, `break`/`continue`**, with indentation | Without loops, no real firmware logic is possible. This is the biggest single unlock | ✅ |
| 2.2 | **Comparisons and booleans**: `== != < <= > >=`, `and or not`, `True`/`False` | Needed by 2.1 | ✅ |
| 2.3 | **Hex and binary literals and bitwise operators**: `0x40`, `0b1010`, `& \| ^ ~ << >>` | Daily work with registers, flags and masks | ✅ |
| 2.4 | **Augmented assignment**: `+= -= \|= &= <<=` | Common in counters and bit manipulation | ✅ |
| 2.5 | **`def` functions** (no closures) | Reusable code such as `blink(n)`. Needs a fixed-size call frame stack | ✅ |
| 2.6 | **`bytes`/`bytearray` from a static buffer pool** | Needed for I²C, SPI and UART payloads, while staying zero-heap. Moved to Phase 3: it depends on the bus API design (`HardwareBridge` passed only integers then) | ✅ `bytes` on the heap (5.2), through the bridge in 6.1; `bytearray` is still open |

Phase 2 follow-ups found while building it:

* ✅ `for` loops with `range()`, the most common loop form in MicroPython code (5.3).
* ✅ `global` statement, so functions can update global counters and state (done, with `abs`/`min`/`max`).
* ✅ `hex()`/`bin()`/`oct()` output for register values (6.1).

## Phase 3: Hardware API (MicroPython-style names)

Start with flat functions that follow MicroPython's names. Move to `machine.Pin(...)`-style objects once attribute access exists.

| # | API | Notes | Status |
|---|-----|-------|--------|
| 3.1 | `sleep_ms(ms)`, `sleep_us(us)`, `ticks_ms()`, `ticks_diff(a, b)` | Same names as MicroPython's `time` module. Keep `delay_ms` as an alias | ✅ |
| 3.2 | `pin_mode(pin, OUT/IN/IN_PULLUP)`, `pin_write(pin, v)`, `pin_read(pin)` | Runtime pin numbers with an allowlist per board, so scripts can't touch the flash or PSRAM pins | ✅ |
| 3.3 | `adc_read(pin)`, `pwm(pin, freq, duty)` | Sensors, LED dimming and servos | ✅ |
| 3.4 | `i2c_write(addr, data)`, `i2c_read(addr, n)`, `i2c_scan()` | `i2c_scan()` is the first thing everyone runs with a new sensor | ✅ (6.2) |
| 3.5 | `uart_write`/`uart_read` and SPI transfers | GPS modules, modems, displays | ✅ (6.2) |
| 3.6 | Watchdog: `wdt_feed()`, plus automatic feeding inside `sleep_ms` | Production firmware has to recover from a hung script | ✅ |
| 3.7 | TFT functions: `cls()`, `color(r, g, b)`, `text(x, y, s)`, `backlight(on)`, and scrolling | Makes the `tft` feature useful beyond showing logs | ✅ |
| 3.8 | **Native modules with `import`**: `import display`, `from time import sleep_ms`; a `display` drawing module (pixels, lines, rectangles, circles, triangles, text) and a `time` module | Rust code grouped into MicroPython-style modules instead of one flat namespace | ✅ |
| 3.9 | **`buttons` module and apps**: debounced short/long presses, `display.number()` and fonts, and a two-button calculator (`scripts/calculator.py`) | Real interactive apps on the board without a PC | ✅ |
| 3.10 | **Calibrated ADC** (`adc_mv`, eFuse Vref), `battery_mv()`, and a voltmeter app (`scripts/voltmeter.py`) | Readings in millivolts instead of raw counts. ESP-IDF's 11 dB lookup table is still missing | ✅ |

Design note (✅ done): `HardwareBridge` now has a function table (`functions() -> &[HwFn]`) with argument counts,
used for checks and `help()`, plus `constants()` and string arguments (`call_args`). Dispatch is still a `match` on the name,
which keeps the trait simple.

Phase 3 status:

* 3.4 and 3.5 are done in Phase 6, with `bytes`: multi-byte I²C, UART1 and SPI3.
* PWM output and the I²C transfers were checked for errors on a TTGO T-Display, but not measured with a scope or a
  real sensor. UART and SPI were checked with loopback wires (`scripts/bus_loopback.py`), not with real devices.

## Phase 4: Developer experience on the device

| # | Feature | Why | Status |
|---|---------|-----|--------|
| 4.1 | **REPL over USB/UART** with line editing and Ctrl-C | The defining MicroPython experience: try code without reflashing | ✅ |
| 4.2 | **Script from flash**: `include_str!` at build time (`make flash SCRIPT=blink.py`), then a data partition with `main.py` | Users change Python code without touching Rust. Done: build-time embedding, and `main.py` saved in the `nvs` partition over USB (`tools/mpy.py save`) | ✅ |
| 4.3 | **Upload a script over serial** (MicroPython's raw REPL: works with `mpremote run/exec` and `tools/mpy.py`) | Fast edit, run, edit cycle | ✅ |
| 4.4 | **Example gallery** in `micropython-esp32/scripts/`: blink, button, ADC, I²C scan, TFT clock | Each one also doubles as a hardware smoke test | ✅ |
| 4.5 | **More boards**: ESP32-C3 and C6 (RISC-V), and an SSD1306 OLED feature | Cheap boards that many people already own | ⬜ Not started: needs the hardware to test on |

Phase 4 follow-ups found while building it:

* ✅ `str()`, string concatenation and formatting (5.1, 5.5).
* ✅ Importing `.py` files as modules (6.3), and a `machine` module with `Pin` objects (7.1).
* ✅ A file system: a flat file system in a `vfs` partition (`src/flatfs.rs`, `tools/mpy.py ls/put/get/rm`), with
  `open()` / `os` for scripts (5.6), so `mpremote cp`/`ls` and Thonny's file browser work.
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

Left for later (documented in the README): sets, generators, `nonlocal`, operator methods (`__add__`,
`__eq__`, ...), multiple inheritance, `bytearray`, step slices (`a[::2]`). (Importing `.py` files: done in 6.3.)

Found while building 5.1:

* **Speed on the ESP32:** with the heap and the shared value stack, tight loops got ~20% slower on the ESP32
  (5000 empty `while` passes: 829 ms before, 1005 ms after) although the host was unchanged, and after 5.8 they
  took 1635 ms: the flash instruction cache. Fixed with the `iram` feature of the core library (the hot loop runs
  from IRAM): 897 ms, with all of Phase 5 in; drawing 50 pixels takes 31 ms (34 ms before Phase 5).

**Later:** once loops and functions exist and profiling shows the evaluator is a bottleneck, add the
bytecode compiler and VM, plus a `const` folding pass.

## Phase 6: Buses and modules (unblocked by Phase 5)

Phase 5's heap made `bytes` possible, which the bus APIs of Phase 3 were waiting for, and its file system made
`import` of `.py` files possible.

| # | Feature | Why | Status |
|---|---------|-----|--------|
| 6.1 | **Bytes through `HardwareBridge`**: `Arg::Bytes` arguments, functions that return bytes (`HwFn::returning_bytes`, `call_bytes`/`call_module_bytes`, up to `MAX_BRIDGE_BYTES` = 512), `bytes([1, 2])` / `bytes(iterable)`, `hex()`/`bin()`/`oct()` | Bus payloads in both directions, without allocations in the bridge (it writes into a heap buffer it's given) | ✅ |
| 6.2 | **Firmware buses**: `i2c_scan()` (addresses as bytes), `i2c_write`, `i2c_read`, `i2c_write_read`; `uart_init/write/read/any` on UART1; `spi_init/write/transfer/read` on SPI3, on any allowed pins | 3.4 and 3.5: sensors, GPS modules, modems, displays | ✅ on a TTGO T-Display (loopback) |
| 6.3 | **`import` of your own `.py` modules** from the file system: own globals per module, cached (`sys.modules`-like), `from m import f`, `m.x = v`, errors don't leave half-imported modules; source read into a fixed module memory (`with_module_memory`, 16 KB on the ESP32) | Splitting a project into files, libraries for sensors | ✅ |
| 6.4 | Docs, `tests/cpython/modules.py`, board checks | | ✅ |

**Phase 6 is done**, on the PC and on a TTGO T-Display:

* UART: a TX-to-RX wire on GPIO25 returns what was sent at 115 200 and 9 600 baud; `uart_any()` counts what's
  waiting, an empty read returns `b''` after the timeout. SPI: a MOSI-to-MISO wire on GPIO27 returns every byte,
  also in a 512-byte transfer. I²C: without devices, `i2c_scan()` is `b''` and `i2c_read` raises `OSError`.
  The flash pins (6–11) are refused. All in `micropython-esp32/scripts/bus_loopback.py`.
* `tests/cpython/modules.py` (modules written to files, then imported) prints what CPython prints, on the PC and
  on the board, like the ten other conformance scripts. Thonny 3.2.7 and 5.0 still pass against the board.
* Found on the board: the REPL's 64 KB of buffers (plus the new 16 KB module memory) came out of the main stack,
  because statics and the stack share the main data RAM. They moved to the ESP32's second data RAM region
  (`dram2`, 96 KB the bootloader leaves free), so the stack got 80 KB back. `MAX_VARS` went from 64 to 128 (deep
  method chains with `**kwargs` ran out of variable slots), and `MAX_NESTING` from 24 to 40, raising
  `RecursionError` like the stack guard instead of `MemoryError`.

Open after Phase 6 (the `machine` module is done in 7.1): `bytearray`
(in-place buffers), packages (`import a.b`), and testing against real sensors (an MPU6050 on I²C, a GPS on UART).

## Phase 7: MicroPython compatibility (developer experience)

| # | Feature | Why | Status |
|---|---------|-----|--------|
| 7.1 | **`machine` module**: `Pin`, `ADC`, `PWM`, `I2C`/`SoftI2C`, `UART`, `SPI`/`SoftSPI`, `WDT`, `reset`, `freq`, `unique_id`, written in Python (`micropython-esp32/modules/machine.py`) and built in as a frozen module (`Interpreter::with_frozen_modules`) | Code from MicroPython tutorials runs unchanged | ✅ on a TTGO T-Display (`scripts/machine_demo.py`) |
| 7.2 | Raw-paste mode | Faster uploads from Thonny and mpremote | ⬜ |
| 7.2a | **CPU clock at build time**: `make flash CPU_MHZ=80/160/240` (80 by default); `machine.freq()` reports it | Scripts run 2× / 3× faster when power doesn't matter | ✅ measured on the TTGO (README, "CPU clock") |
| 7.4 | **Floats** (double precision) and the `math` module, `float()`, `round()`, `divmod()`, `pow()`, `%f %e %g` formatting, `time.sleep(0.5)` | Sensor maths in volts, degrees, percent; MicroPython tutorials use floats everywhere | ✅ identical to CPython (`tests/cpython/floats.py`) on the PC and on the TTGO |
| 7.5 | **Single precision floats** (`float32`, `make flash FLOAT=32`), like MicroPython's ESP32 port | Its digits and rounding; the FPU | ✅ measured on the TTGO (README, "Single precision") |
| 7.6 | **Wi-Fi: `network.WLAN`** (station mode) like MicroPython's ESP32 port: `active`, `scan`, `connect`, `isconnected`, `status`, `ifconfig`, `config`, `disconnect` (`FEATURES=wifi`) | Connected boards; the base for sockets and HTTP | ✅ on a TTGO with a WPA2 network |
| 7.7 | **`Pin.irq()` and `machine.Timer`**: soft interrupts like MicroPython's (callbacks between statements, during sleeps and at the prompt) | Buttons, periodic tasks, event-driven code from MicroPython tutorials | ✅ on a TTGO (edges on a looped-back pin, timers) |
| 7.8 | **Wi-Fi access point**: `network.WLAN(network.AP_IF)` with `config(ssid=, key=, authmode=, channel=, hidden=, max_clients=)`, `ifconfig()`, `status('stations')`, alone or with the station; a DHCP server in the firmware | The board's own network for setup pages and phones | ✅ on a TTGO, joined from a Linux laptop |
| 7.3 | **Feature levels** (`minimal`, `medium`, `full`; `full` by default) for the core library and the firmware (`make flash LEVEL=...`) | Smaller builds and less RAM for small chips | ✅ (README, "Feature levels") |

Done for 7.1:

* New in the interpreter: frozen modules (a `machine.py` on the file system comes first, as in MicroPython),
  hardware functions as values (`f = pin_write`, `reset = reset`; the script's own functions shadow them),
  `__call__` (`led()`, `led(1)`), and `MAX_FUNCS` 64 (the module takes 43 slots; 6 KB more stack on the ESP32).
* New in the firmware: open-drain pins (`OUT_OD`, `OUT_OD_PULLUP`), `pwm_u16` (16-bit duty, for `duty_u16` and
  servo pulses in `duty_ns`), `i2c_init(sda, scl, freq)`, `unique_id()` (the MAC address), `cpu_freq()`.
* Tested on the PC against a mock board that logs every hardware call (`tests/machine_module.rs`), and on the TTGO:
  blinking, the BOOT button, ADC, PWM, I²C scan, UART `readline()` and SPI over loopback wires, the watchdog.
* Found while writing it: a parameter with a default followed by `*args` or `**kwargs` (`def f(a, b=1, **kw)`) was a
  syntax error (fixed, in `tests/cpython/functions.py` now), and `I2C(...)` hung the board: the old I2C driver was
  dropped after the new one was made, which reset the peripheral under it.
* Still missing: (`Pin.irq` and `Timer`: done in 7.7) `RTC`, `Signal`, the `*_into` methods (need `bytearray`), `time.sleep(0.5)`
  (floats: done in 7.4), keyword-only parameters (`def f(*, x)`), and unpacking into attributes (`self.a, self.b = a, b`). One
  bus per kind (I²C, UART, SPI). A method call takes 1.2 ms at 80 MHz (`pin_write` 0.27 ms); `CPU_MHZ=240` (7.2a)
  makes that 0.43 ms. Changing the clock while running (`machine.freq(f)`, by saving it and rebooting) is open.

Done for 7.3:

* Cargo features `minimal`, `medium`, `full` (default), made of the parts `classes`, `format`, `files`, `modules`
  and `stdlib`. A missing part is replaced by a stub with the same entry points (`src/off/`), so its code isn't in
  the binary and the rest of the interpreter has no `#[cfg]` at its call sites; the limits (functions, variables,
  call depth, nesting, value stack) shrink with the level. Using a missing feature raises `NotImplementedError`.
* ESP32, generic build: 258 / 278 / 304 KB app (356 / 380 KB for `medium` / `full` with floats, 7.4), 10.0 / 15.5 / 22.7 KB for the interpreter in RAM, 29 / 35 / 42 KB of
  stack under a top-level statement. Checked on the TTGO at each level (conformance scripts, stack, speed).
* CI: `make levels` (lints, tests and the ARM build of `minimal` and `medium`), and a `minimal` firmware build.
* Not measured yet: the size on a Cortex-M chip (only the ARM build is checked).

Done for 7.4:

* `Value::Float(f64)`, float literals in the lexer, and the float paths in `src/floats.rs` (a stub in `minimal`):
  Python's arithmetic (CPython's `float_divmod` for `//` and `%`), NaN-aware comparisons, `repr()` as the shortest
  round-trip text laid out like CPython, `float()`, `round()` (half to even, correctly rounded `round(x, n)`),
  formatting types `f e g %` for `%`, `str.format` and f-strings, and the `math` module over `libm`.
* Like MicroPython: ints are converted to floats in mixed operations; `sum()` adds plainly. Like CPython (and
  MicroPython's double builds): 17 significant digits.
* On the ESP32: `floats.py` and all other conformance scripts identical to CPython; 86 KB more flash, no more RAM;
  1000 float additions 209 ms (ints 171 ms); int-only benchmark 2 % slower. Fuzzed with float tokens.
* Not done: complex numbers, floats returned by hardware functions (`HardwareBridge::call` returns ints).

Done for 7.5: `micropython_rs::Float` is `f64`, or `f32` with the `float32` feature; libm's `f32` functions;
`repr()` with 7 significant digits like MicroPython. On the ESP32 the maths is 2-11× faster natively (FPU instead
of the ROM's soft-float), but Python code only 0-13 % (the interpreter's own overhead is 35-100 times a soft-float
operation); 6 KB less flash, no RAM change. All measurements (native and Python, both builds) are in the README,
"Single precision".

Done for 7.6:

* `esp-radio` 0.17 (Espressif's Wi-Fi driver) on `esp-rtos` 0.2, both started by the first `wlan.active(True)`;
  `smoltcp` for DHCP and IP, polled when scripts ask and while they sleep (no background task); a 64 KB heap for
  the driver (16 KB in `dram2`, 48 KB of the main RAM). `network` is a frozen module (`modules/network.py`) over
  the firmware's `wifi_*` functions; `tests/network_module.rs` checks it against a mock radio.
* On the TTGO: scan 0.5-1 s, connected with DHCP in 2.7-5.3 s, `STAT_WRONG_PASSWORD` / `STAT_NO_AP_FOUND`
  reported, pings answered, the connection kept over soft resets and Thonny sessions; everything else unchanged
  (Thonny, conformance, buses), 2 % slower while connected.
* Costs: +435 KB flash, +53 KB IRAM, 70 KB of the stack (111 KB left), so it's a feature (`FEATURES=wifi`), not
  in every build; CI builds it with a 96 KB stack minimum.
* Next: `socket` (TCP/UDP over the same smoltcp interface), then `urequests`; static addresses. (Access point mode:
  done in 7.8.)

Done for 7.8:

* One driver configuration for the station, the access point or both (`ModeConfig::ApSta`); a second smoltcp
  interface at 192.168.4.1/24 with a DHCP server in the firmware (DISCOVER/OFFER, REQUEST/ACK/NAK, RELEASE;
  192.168.4.2 and up, 2-hour leases); joined devices from the driver's events; `network.py` with `AP_IF`.
* The network is also polled while the REPL waits for input (raw REPL included), so the access point keeps serving
  after a script ended and under Thonny.
* Found under load (both interfaces, a device joined, pings, scans): the driver's default buffers took the 64 KB heap
  to its limit. Now 6 + 12 receive and 12 transmit buffers and a 72 KB heap: 55 KB at most; the stack is 102 KB.
* Tested from a Linux laptop: WPA2 and open networks found, address 192.168.4.2 by DHCP, 50/50 pings (2.8-27 ms),
  1400-byte pings, both interfaces answering at once; the laptop's wired default route untouched.

Done for 7.7:

* Core: `HardwareBridge::poll_event` (events by number), `set_events`, `resume_sleep` and `soft_reset`, and
  `Error::Pending`: a sleep that sees an event stops early with the time left; the interpreter runs the handler and
  resumes the sleep (nothing kept in the firmware, so a handler may sleep too). The handler is one Python function
  (`_irq_handler(f)`), called between statements (in `tick`, with the abort and step checks) and by
  `Interpreter::run_pending_events`, which the REPL calls at the `>>>` prompt. Handlers don't nest; errors are
  printed, Ctrl-C stops the script.
* Firmware: a GPIO interrupt handler that records the pins (`events.rs`), software timers (ids 0-3 and -1), sleeps in
  1 ms steps while events may come, and a soft reset that stops them. `machine.py`: `Pin.irq()`, `Timer`, and the
  dispatcher from event numbers to callbacks.
* `MAX_FUNCS` 96 for the `full` level (`machine` now takes 48 slots): 6 KB more RAM for the interpreter (29 KB REPL
  frame). Building the interpreter out of line (`from_parts`) keeps its temporary copies off the scripts' stack.
* Measured on the TTGO: a 50 ms timer fires 20 times in `sleep_ms(1000)`, a 100 Hz one 50 times in a 500 ms busy
  loop, both-edge interrupts give 10 callbacks for 5 pulses (values right), timers keep going at the prompt and stop
  on a soft reset, also with Wi-Fi connected; 6 % slower with a timer set, unchanged otherwise.
* Not done: level triggers, hard interrupts (Python in the interrupt itself), `RTC`, `Signal`,
  `micropython.schedule()`.

## Recommended order

1. **1.1, 1.2, 1.4**: calls in expressions, line numbers, step limit. Small, and later work depends on them.
2. **2.1–2.3**: control flow, comparisons, bitwise operators. After this, real firmware scripts become possible.
3. **3.1–3.2 and 4.2**: time, GPIO, script from a file. A usable product: a `main.py` that blinks, reads buttons and loops.
4. **4.1**: REPL. Then I²C and ADC (3.3–3.4), with fuzzing and firmware CI (1.3, 1.5) in parallel.
