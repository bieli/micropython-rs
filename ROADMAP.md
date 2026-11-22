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
* `global` statement, so functions can update global counters and state.
* `hex()`/`bin()` output for register values (needs a small string pool, like 2.6).

## Phase 3: Hardware API (MicroPython-style names)

Start with flat functions that follow MicroPython's names. Move to `machine.Pin(...)`-style objects once attribute access exists.

| # | API | Notes | Status |
|---|-----|-------|--------|
| 3.1 | `sleep_ms(ms)`, `sleep_us(us)`, `ticks_ms()`, `ticks_diff(a, b)` | Same names as MicroPython's `time` module. Keep `delay_ms` as an alias | ⬜ |
| 3.2 | `pin_mode(pin, OUT/IN/IN_PULLUP)`, `pin_write(pin, v)`, `pin_read(pin)` | Runtime pin numbers with an allowlist per board, so scripts can't touch the flash or PSRAM pins | ⬜ |
| 3.3 | `adc_read(pin)`, `pwm(pin, freq, duty)` | Sensors, LED dimming and servos | ⬜ |
| 3.4 | `i2c_write(addr, data)`, `i2c_read(addr, n)`, `i2c_scan()` | `i2c_scan()` is the first thing everyone runs with a new sensor. Needs 2.6 | ⬜ |
| 3.5 | `uart_write`/`uart_read` and SPI transfers | GPS modules, modems, displays | ⬜ |
| 3.6 | Watchdog: `wdt_feed()`, plus automatic feeding inside `sleep_ms` | Production firmware has to recover from a hung script | ⬜ |
| 3.7 | TFT functions: `cls()`, `color(r, g, b)`, `text(x, y, s)`, `backlight(on)`, and scrolling | Makes the `tft` feature useful beyond showing logs | ⬜ |

Design note: replace the `match` on string names in `HardwareBridge` with a **static function table**
(`&[(&str, arity, fn)]`). Each board adds functions in one place, the core can report the wrong number of
arguments clearly, and a `help()` listing can be generated from it.

## Phase 4: Developer experience on the device

| # | Feature | Why | Status |
|---|---------|-----|--------|
| 4.1 | **REPL over USB/UART** with line editing and Ctrl-C | The defining MicroPython experience: try code without reflashing | ⬜ |
| 4.2 | **Script from flash**: `include_str!` at build time (`make flash SCRIPT=blink.py`), then a data partition with `main.py` | Users change Python code without touching Rust | ⬜ |
| 4.3 | **Upload a script over serial** (simple protocol or raw-paste mode) | Fast edit, run, edit cycle | ⬜ |
| 4.4 | **Example gallery** in `examples/`: blink, button, ADC, I²C scan, TFT clock | Each one also doubles as a hardware smoke test | ⬜ |
| 4.5 | **More boards**: ESP32-C3 and C6 (RISC-V), and an SSD1306 OLED feature | Cheap boards that many people already own | ⬜ |

**Later:** once loops and functions exist and profiling shows the evaluator is a bottleneck, add the
bytecode compiler and VM, plus a `const` folding pass.

## Recommended order

1. **1.1, 1.2, 1.4**: calls in expressions, line numbers, step limit. Small, and later work depends on them.
2. **2.1–2.3**: control flow, comparisons, bitwise operators. After this, real firmware scripts become possible.
3. **3.1–3.2 and 4.2**: time, GPIO, script from a file. A usable product: a `main.py` that blinks, reads buttons and loops.
4. **4.1**: REPL. Then I²C and ADC (3.3–3.4), with fuzzing and firmware CI (1.3, 1.5) in parallel.
