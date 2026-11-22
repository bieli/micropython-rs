# micropython-rs

![CI status](https://github.com/bieli/micropython-rs/actions/workflows/ci.yml/badge.svg)
![github_tag](https://img.shields.io/github/v/tag/bieli/micropython-rs)
[![Crates.io](https://img.shields.io/crates/v/micropython_rs.svg)](https://crates.io/crates/micropython_rs)
[![docs.rs](https://img.shields.io/docsrs/micropython_rs)](https://docs.rs/micropython_rs)


MicroPython virtual machine written in RUST language for embedded systems


# Intro

A minimal, zero-dependency, `#![no_std]` Python interpreter written in Rust.

Designed from scratch to run on bare-metal microcontrollers (like ARM Cortex-M) **without** needing the Rust standard library (`std`) or dynamic heap allocations.

---

## Why This Project?

If you are new to Rust or embedded development, this codebase is a clean, readable starting point to learn how interpreters work under the hood:

* **Zero Heap Allocations:** Uses stack arrays and string slices (`&str`) instead of `Vec` or `String`.
* **Hardware Agnostic (`#![no_std]`):** Connects to any display, UART serial port, or terminal via a simple 1-method `Output` trait.
* **Modular & Easy to Read:** Split into four small, self-contained files (`lexer.rs`, `ast_parser.rs`, `interpreter.rs`, and `lib.rs`).

---

## Quick Start (For Rust Beginners)

### 1. Prerequisites
Install Rust using rustup, then add the ARM Cortex-M target so you can test bare-metal builds:

```bash
rustup target add thumbv7em-none-eabihf
```

2. Run hello.py example
```bash
make run FILE=hello.py
```

3. Run all tests
```bash
make all
```


## Supported Python subset

```python
STATUS_READY = 0x01               # hex, octal (0o17), binary (0b1010) literals, 1_000 separators

def wait_ready(max_tries):        # functions with parameters, return values, recursion
    tries = 0
    while tries < max_tries:      # while / break / continue
        if read_reg(0x10) & STATUS_READY:   # hardware calls return values
            return True
        tries += 1
        delay_ms(10)
    return False

if wait_ready(5) and not error_flag():   # and / or / not, True / False / None
    print('sensor ready')
elif 0 < retries <= 3:            # chained comparisons
    print('retrying')
else:
    print('giving up')

flags = 0
flags |= 1 << 3                   # bitwise & | ^ ~ << >> and augmented assignment
```

| Area | Supported |
|------|-----------|
| Values | `int` (64-bit), `str` (literals only), `bool` (`True`/`False`), `None` |
| Literals | `42`, `1_000`, `0x1F`, `0o17`, `0b1010`, `'single'`, `"double"` |
| Operators | `+ - * / // % **`, `& \| ^ ~ << >>`, `== != < <= > >=` (chainable), `and or not` |
| Assignment | `x = ...`, and `+= -= *= /= //= %= **= &= \|= ^= <<= >>=` |
| Statements | `if`/`elif`/`else`, `while`, `break`, `continue`, `pass`, `def`, `return` |
| Calls | `print(...)`, your own `def` functions, and hardware functions from `HardwareBridge` |

Not supported yet: `for`, lists/dicts/tuples, floats, string operations, `global`, classes, imports.
See [ROADMAP.md](ROADMAP.md) for what's planned.

### Arithmetic operators

All arithmetic is on 64-bit integers (`i64`). Precedence goes from lowest to highest, as in Python:

| Precedence | Operators | Associativity | Example |
|------------|-----------|---------------|---------|
| lowest | `or` | left, short-circuit | `0 or 5` → `5` |
| | `and` | left, short-circuit | `3 and 0` → `0` |
| | `not` | | `not a == b` → `not (a == b)` |
| | `==` `!=` `<` `<=` `>` `>=` | chained | `1 < x < 10` → `1 < x and x < 10` |
| | `\|` | left | |
| | `^` | left | |
| | `&` | left | `0xFF & 1 << 4` → `16` |
| | `<<` `>>` | left | `1 << 2 + 1` → `8` |
| | `+` `-` | left | `10 - 4 - 3` → `3` |
| | `*` `/` `//` `%` | left | `1 + 2 * 3` → `7` |
| | unary `-` `+` `~` | right | `-a * 3` → `(-a) * 3` |
| highest | `**` | right | `2 ** 3 ** 2` → `512`, `-2 ** 2` → `-4` |
| | `( ... )` | | `(1 + 2) * 3` → `9` |

Rules (they match CPython unless marked *differs*):

* `//` rounds toward −∞ (`-7 // 2` → `-4`), and `%` takes the sign of the divisor (`-7 % 3` → `2`).
* *Differs:* `/` is integer (floor) division (`7 / 2` → `3`), because floats aren't supported yet.
* *Differs:* results that don't fit in `i64` raise `Error::Overflow`. CPython has unlimited-size integers.
* `x / 0`, `x // 0` and `x % 0` raise `Error::ZeroDivision`.
* Negative exponents (`2 ** -1`) and arithmetic on strings (`'a' + 'b'`) raise `Error::TypeError`.
  String concatenation would need heap memory.
* `bool` behaves as an integer (`True + 1` → `2`); `&`, `|`, `^` of two bools give a bool.
* `and`/`or` return one of their operands and skip the right side when the result is known,
  so `0 and f()` never calls `f`.
* Strings compare with each other (`'a' < 'b'`). `==`/`!=` between different types is `False`/`True`;
  ordering them (`1 < 'a'`) raises `Error::TypeError`.
* *Differs:* `<<` that doesn't fit in `i64` raises `Error::Overflow`; a negative shift count raises `Error::ValueError`.

How it works without a heap: the parser turns each expression into postfix (RPN) order in a fixed-size buffer.
For example, `(a + 2) * -3` becomes `[a, 2, Add, 3, Neg, Mul]`.
The interpreter then evaluates it with a small fixed-size value stack.
The limits are in `src/ast_parser.rs`:

| Constant | Value | Meaning |
|----------|-------|---------|
| `MAX_EXPR_ITEMS` | 32 | Operands and operators per statement, all call arguments included |
| `MAX_EXPR_DEPTH` | 16 | Nesting of parentheses and unary operators |
| `MAX_CALL_ARGS` | 8 | Arguments per function call (and parameters per `def`) |

Going over a limit raises `Error::OutOfMemory`.

### Control flow and functions

Blocks are defined by indentation, as in Python (a tab counts to the next multiple of 8 columns).
A body can also be a single statement on the same line: `if x: led_on()`.

```python
def blink(times, ms):
    n = 0
    while n < times:
        led_on()
        delay_ms(ms)
        led_off()
        delay_ms(ms)
        n += 1

blink(3, 250)
```

* Function parameters and variables assigned inside a function are **local**. Globals can be read.
  There's no `global` keyword yet, so `counter += 1` on a global inside a function raises `Error::UnknownIdentifier`.
* Functions are defined at the top level (or inside `if`/`while` at the top level). A `def` inside a `def` raises `Error::InvalidSyntax`.
* Calling with the wrong number of arguments raises `Error::TypeError`.
* `break`/`continue` outside a loop and `return` outside a function raise `Error::InvalidSyntax`,
  even in code that never runs, as Python does at compile time.
* Bad indentation raises `Error::IndentationError`.

How it works without a heap: the interpreter runs the source while parsing it and never builds a program in memory.
A `while` loop keeps a copy of the parser (96 bytes) from its header and jumps back to it after each pass.
A `def` stores a parser copy that points at its body. A block that doesn't run is still parsed, so syntax errors are always reported.
The trade-off is speed: loop bodies are parsed again on every pass. The planned bytecode compiler will remove that cost.

Limits (in `src/interpreter.rs`):

| Constant | Value | Meaning | Error |
|----------|-------|---------|-------|
| `MAX_VARS` | 32 | Variables: globals plus the locals of all active calls | `OutOfMemory` |
| `MAX_FUNCS` | 8 | `def` functions | `OutOfMemory` |
| `MAX_CALL_DEPTH` | 8 | Nested function calls, recursion included | `RecursionLimit` |
| `MAX_NESTING` | 24 | Nested blocks (`if`/`while`/function bodies) running at once | `OutOfMemory` |

Native stack use grows with each nested call (an RPN buffer and value stack per call), which is why
`MAX_CALL_DEPTH` is small. On a 64-bit host the `Interpreter` struct is about 3.4 KB; on 32-bit MCUs it's smaller.

### Function calls

Calls are expressions, so they can be nested and combined with operators: `print(f(g(1), 2 + h()))`.

* `print(...)` is built in. It prints its arguments separated by spaces and returns `None`.
* Your own `def` functions are looked up next.
* Every other name is passed to your `HardwareBridge`. Arguments must be integers or bools (otherwise `Error::TypeError`).
  Return `Ok(Some(n))` to give the script a value, or `Ok(None)` for `None`.
* All arguments are evaluated before the call runs. If one fails, nothing is printed or sent to hardware.

### Errors, line numbers and running safely on a device

`eval()` returns an `Error`, and `error_line()` tells you where it happened:

```rust
if let Err(err) = vm.eval(script) {
    match vm.error_line() {
        Some(line) => println!("Python Error at line {}: {:?}", line, err),
        None => println!("Python Error: {:?}", err),
    }
}
```

Syntax errors point at the offending token, and runtime errors at the statement that failed.

Two guards stop a script that runs too long, such as an accidental `while True:`:

```rust
use core::sync::atomic::{AtomicBool, Ordering};

static ABORT: AtomicBool = AtomicBool::new(false);   // set to true from a UART/button interrupt

vm.set_step_limit(Some(10_000));  // eval() returns Error::StepLimit after 10 000 statements/loop passes
vm.set_abort_flag(&ABORT);        // eval() returns Error::Interrupted once ABORT is true
```

The step budget starts over for each `eval()` call. The abort flag is checked before every statement and loop pass.
It's only read, so clear it yourself before the next run. The ESP32 firmware sets a step limit of 1 000 000.

## Using micropython-rs on a Microcontroller

Because the core library is #![no_std], you can embed it into any firmware project 
by implementing the Output trait for your hardware interface (UART, USB serial, LCD screen, etc.):

```rust
#![no_std]

use micropython_rs::{Interpreter, Output};

struct UartWriter;

impl Output for UartWriter {
    fn write_str(&mut self, s: &str) {
        // Send string bytes over your microcontroller's UART peripheral
    }
}

fn run_script() {
    let mut uart = UartWriter;
    let mut vm = Interpreter::new(&mut uart);
    let _ = vm.eval("print('Hello from bare metal!')");
}
```

## Running on real ESP32 hardware

The [`micropython-esp32/`](micropython-esp32) folder holds the firmware. It runs Python code on ESP32 (Xtensa) boards.
You pick the board at build time with a **Cargo feature**.

### Supported boards (`FEATURES`)

| Board | Cargo feature | Make command | Python `print()` goes to | Python hardware functions |
|-------|---------------|--------------|--------------------------|---------------------------|
| Any ESP32 board (DevKit, WROOM, WROVER, ...) | *(none, default)* | `make flash` | USB/UART serial monitor | `led_on()`, `led_off()`, `delay_ms(ms)` |
| LilyGO TTGO T-Display (ST7789 1.14" TFT) | `tft` | `make flash FEATURES=tft` | TFT screen **and** serial monitor | `delay_ms(ms)` |

> [!IMPORTANT]
> On a TTGO T-Display, always build with `FEATURES=tft`. The default build switches GPIO4 off as the "LED" pin,
> and on this board GPIO4 is the display backlight, so the screen stays dark.

### Quick start (Docker + Make)

Run these in the `micropython-esp32/` folder. The build runs inside the official `espressif/idf-rust:esp32_latest`
Docker image, so you don't need the Xtensa toolchain on your machine. Flashing and the monitor run on the host with `espflash`.

```bash
cd micropython-esp32

make build                         # generic ESP32 firmware (serial console + LED)
make build FEATURES=tft            # LilyGO TTGO T-Display firmware (TFT console)

make flash                         # build, flash and open the serial monitor (generic)
make flash FEATURES=tft            # build, flash and open the serial monitor (TTGO T-Display)
make flash PORT=/dev/ttyUSB0       # pick a serial port explicitly

make monitor                       # open the serial monitor only (CTRL+R reset, CTRL+C exit)
make clean                         # remove build artifacts
```

| Make variable | Default | Meaning |
|---------------|---------|---------|
| `FEATURES` | *(empty)* | Cargo features passed as `--features ...` (e.g. `tft`) |
| `PORT` | *(empty, espflash picks the saved port)* | Serial port for `flash` / `monitor` |

Without Docker/Make (the `esp` toolchain installed locally with `espup`):

```bash
cargo build --release                     # generic ESP32
cargo build --release --features tft      # TTGO T-Display
espflash flash --monitor target/xtensa-esp32-none-elf/release/micropython-esp32
```

### Implementation details

#### Firmware layout

| File | Purpose |
|------|---------|
| `micropython-esp32/src/bin/main.rs` | Firmware entry point. Each board has its own `#[cfg(feature = ...)]` setup and demo Python script, and both share a `run()` helper |
| `micropython-esp32/src/serial_console.rs` | `EspSerialOut`, an `Output` that writes `print()` text to USB/UART with `esp_println` (always compiled) |
| `micropython-esp32/src/tft_console.rs` | `TftConsole`, an `Output` that draws text on the ST7789 TFT (only compiled with `tft`) |
| `micropython-esp32/Cargo.toml` | Declares the `tft` feature. `mipidsi`, `embedded-graphics` and `embedded-hal-bus` are optional dependencies, so boards without a display don't pull them in |
| `micropython-esp32/Makefile` | Docker build and host flash/monitor, with the `FEATURES` and `PORT` variables |

#### How Python talks to the hardware

The interpreter core (`micropython-rs`) has no hardware code of its own. The firmware plugs hardware in through two traits:

* **`Output`** decides where `print()` text goes. It is `EspSerialOut` (serial) or `TftConsole` (screen + serial).
* **`HardwareBridge`** handles calls to functions the interpreter doesn't know, such as `led_on()` or `delay_ms(500)`.
  The firmware matches the function name and its integer arguments, and returns `Error::UnknownIdentifier` for anything else.

```rust
impl HardwareBridge for Esp32Board<'_> {
    fn call(&mut self, name: &str, args: &[i64]) -> Result<Option<i64>, Error> {
        match (name, args) {
            ("led_on", [])  => { self.led.set_high(); Ok(None) }
            ("led_off", []) => { self.led.set_low();  Ok(None) }
            ("delay_ms", [ms]) if *ms >= 0 => { self.delay.delay_millis(*ms as u32); Ok(None) }
            _ => Err(Error::UnknownIdentifier),
        }
    }
}
```

#### Generic ESP32 build (default)

* The onboard LED is on **GPIO2**, which is common on ESP32 DevKit boards.
  If your board uses another pin, change `peripherals.GPIO2` in `src/bin/main.rs`.
* `print()` output appears in the serial monitor (`make monitor`).

#### TTGO T-Display build (`--features tft`)

* The display driver is `mipidsi` (ST7789) on SPI2 at 26 MHz in SPI mode 0, with a 512-byte SPI buffer.
* The screen runs in landscape at **240 × 135 px** (rotated 90°, panel offset 52/40, inverted colors).
* Text uses `FONT_6X10` in a 6 × 12 px cell with a 4 px margin, which gives **38 columns × 10 rows**, light green on black.
* `\n` starts a new line, `\r` returns to column 0, and long lines wrap. When all 10 rows are full, the screen clears and writing starts again at the top. It does not scroll.
* All output is copied to the serial monitor as well.
* GPIO4 is used by the backlight, so this build has no `led_on()`/`led_off()`. Only `delay_ms(ms)` is available.

| TFT signal | ESP32 GPIO |
|------------|------------|
| SCLK | GPIO18 |
| MOSI | GPIO19 |
| CS   | GPIO5  |
| DC   | GPIO16 |
| RST  | GPIO23 |
| Backlight | GPIO4 |

#### Adding your own board

1. Add a feature in `micropython-esp32/Cargo.toml`, for example `myboard = []`, plus any optional `dep:` crates it needs.
2. If the board needs its own output (LCD, OLED, ...), add a module that implements `micropython_rs::Output`, and gate it in `src/lib.rs` with `#[cfg(feature = "myboard")]`.
3. In `src/bin/main.rs`, add a `#[cfg(feature = "myboard")]` `main()` that sets up the peripherals, a `HardwareBridge` with your functions, and a `SCRIPT`. Then call `run(...)`.
4. Build it with `make flash FEATURES=myboard`.

### Toolchain setup (first time only)

1. Before prerequisites
```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc

export PATH="$HOME/.cargo/bin:$PATH"
espup install
```

2. Check versions
```bash
$ espflash --version
espflash 4.7.0

$ esp-generate --version
esp-generate 2.0.0
```

3. (Only needed for a brand-new project; this repo already includes it.) How the `micropython-esp32` project was generated
```bash
esp-generate --headless -o esp32 -o esp-backtrace micropython-esp32
```

4. If the Rust version on your OS and the Xtensa one don't match
```bash
mv ~/.rustup/toolchains/esp/bin/cargo ~/.rustup/toolchains/esp/bin/cargo.bak
ln -s "$(rustup which --toolchain stable cargo)" ~/.rustup/toolchains/esp/bin/cargo
```

5. If you still have Rust version problems, use the Docker build (this is what `make build` does)
```bash
docker pull espressif/idf-rust:esp32_latest

docker run --rm \
  -v "$(pwd)":/workspace \
  -w /workspace/micropython-esp32 \
  espressif/idf-rust:esp32_latest \
  bash -c "source /home/esp/export-esp.sh && cargo build --release"   # add: --features tft
```

### Example output

Generic ESP32 build (`make flash`). The interpreted Python output is at the end:
```bash
[2026-11-22T12:53:30Z INFO ] Serial port: '/dev/ttyACM0'
[2026-11-22T12:53:30Z INFO ] Connecting...
[2026-11-22T12:53:37Z INFO ] Using flash stub
Chip type:         esp32 (revision v1.1)
Crystal frequency: 40 MHz
Flash size:        16MB
Features:          WiFi, BT, Dual Core, 240MHz, VRef calibration in efuse, Coding Scheme None
MAC address:       f8:b3:b7:a1:7f:8c
App/part. size:    83,344/16,384,000 bytes, 0.51%
[00:00:02] [========================================]       2/2       0x10000  Verifying... OK!                                                                                       [2026-11-22T12:53:40Z INFO ] Flashing has completed!
Commands:
    CTRL+R    Reset chip
    CTRL+C    Exit

ets Jun  8 2016 00:22:57

rst:0x1 (POWERON_RESET),boot:0x13 (SPI_FAST_FLASH_BOOT)
configsip: 0, SPIWP:0xee
clk_drv:0x00,q_drv:0x00,d_drv:0x00,cs0_drv:0x00,hd_drv:0x00,wp_drv:0x00
mode:DIO, clock div:2
load:0x3fff0040,len:6072
load:0x40078000,len:15776
load:0x40080400,len:3920
entry 0x40080638
I (27) boot: ESP-IDF v6.1-829-g9a97f6c54ec 2nd stage bootloader
I (27) boot: chip revision: v1.1
I (27) boot.esp32: SPI Speed      : 40MHz
I (29) boot.esp32: SPI Mode       : DIO
I (32) boot.esp32: SPI Flash Size : 16MB
I (36) boot: Partition Table:
I (39) boot: ## Label            Usage          Type ST Offset   Length
I (45) boot:  0 nvs              WiFi data        01 02 00009000 00006000
I (51) boot:  1 phy_init         RF data          01 01 0000f000 00001000
I (58) boot:  2 factory          factory app      00 00 00010000 00fa0000
I (65) esp_image: segment 0: paddr=00010020 vaddr=3f400020 size=02084h (  8324) map
I (75) esp_image: segment 1: paddr=000120ac vaddr=3ffb0000 size=004cch (  1228) load
I (80) esp_image: segment 2: paddr=00012580 vaddr=40080000 size=013b8h (  5048) load
I (89) esp_image: segment 3: paddr=00013940 vaddr=00000000 size=0c6d8h ( 50904) 
I (112) esp_image: segment 4: paddr=00020020 vaddr=400d0020 size=04540h ( 17728) map
I (120) boot: Loaded app from partition at offset 0x10000

=== micropython-rs ===
Hello from ESP32
a = 21 b = 21
(a+b)*3-2**4//2 = 118
square 1 = 1 odd
square 2 = 4 even
square 3 = 9 odd
Blinking onboard LED 3 times...
LED sequence complete!
=== Execution complete ===
```

TTGO T-Display build (`make flash FEATURES=tft`). The Python `print()` lines are also shown on the TFT screen (`=== Execution complete ===` goes to serial only):
```bash
...
I (125) boot: Loaded app from partition at offset 0x10000
=== micropython-rs ===
Board: TTGO T-Display
a = 21 b = 21
(a+b)*3-2**4//2 = 118
square 1 = 1 odd
square 2 = 4 even
square 3 = 9 odd
TFT console active!
=== Execution complete ===
```


## TODO list

- [x] Zero-copy Lexer (print, strings, integers, comments)
- [x] Stack-allocated AST Parser & Evaluator
- [x] #![no_std] ARM Cortex-M build verification
- [x] add assiggment in interpreted Python code "="
- [x] running external hello.py file with Python code by interpreter (instead of hardcoded version!)
- [x] running Python interpreted code on ESP32 real hardware by USB/UART monitor
- [x] blinking LED on ESP32 hardware from MicroPython interpreter written in RUST
- [x] Python `print()` on LilyGO TTGO T-Display TFT screen (`--features tft`)
- [x] board selection with Cargo features (generic ESP32 / TTGO T-Display)
- [x] Variables and basic arithmetic (+, -, *, /, //, %, ** with Python precedence and parentheses)
- [x] Control flow (if / elif / else, while, break / continue, with indentation tracking)
- [x] Comparisons, booleans, `and` / `or` / `not`, hex / binary literals, bitwise operators, augmented assignment
- [x] `def` functions with parameters, local variables, `return` and recursion
- [ ] Compact Bytecode Compiler & Stack VM


## Contributing

Contributions are welcome! Whether you are fixing a typo, adding a new Python built-in, or improving docs for fellow Rust learners, feel free to open an issue or submit a pull request.


## License

Licensed under either of Apache License, Version 2.0

