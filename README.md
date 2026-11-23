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
| Statements | `if`/`elif`/`else`, `while`, `break`, `continue`, `pass`, `def`, `return`; `;` between statements (`x = 1; y = 2`) |
| Calls | `print(...)`, `abs(x)`, `min(a, b, ...)`, `max(a, b, ...)`, your own `def` functions, and hardware functions from `HardwareBridge` |
| Scope | `global a, b` in a function, to assign globals (also creates them) |
| Modules | `import m`, `import m as x, n`, `from m import f, C as K`; `m.f(...)`, `m.CONST` for native modules written in Rust |

Not supported yet: `for`, lists/dicts/tuples, floats, string operations, classes, importing `.py` files.
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
  To assign a global, declare it first, as in Python:

  ```python
  count = 0
  def press():
      global count
      count += 1
  ```

  Without `global`, `count += 1` inside the function raises `Error::UnknownIdentifier` (Python: `UnboundLocalError`).
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
| `MAX_VARS` | 64 | Variables: globals plus the locals of all active calls | `OutOfMemory` |
| `MAX_FUNCS` | 16 | `def` functions | `OutOfMemory` |
| `MAX_GLOBAL_DECLS` | 16 | Names declared `global` by all active calls together | `OutOfMemory` |
| `MAX_CALL_DEPTH` | 8 | Nested function calls, recursion included | `RecursionLimit` |
| `MAX_NESTING` | 24 | Nested blocks (`if`/`while`/function bodies) running at once | `OutOfMemory` |

Native stack use grows with each nested call (an RPN buffer and value stack per call), which is why
`MAX_CALL_DEPTH` is small. On a 64-bit host the `Interpreter` struct is about 7 KB; on 32-bit MCUs it's smaller.

### Function calls

Calls are expressions, so they can be nested and combined with operators: `print(f(g(1), 2 + h()))`.

* `print(...)` is built in. It prints its arguments separated by spaces and returns `None`.
* Your own `def` functions are looked up next.
* Every other name is passed to your `HardwareBridge` (see [Connecting hardware](#connecting-hardware-hardwarebridge)).
  Arguments are integers, bools (passed as 0/1) or strings; `None` is a `TypeError`.
  The bridge returns `Ok(Some(n))` to give the script a value, or `Ok(None)` for `None`.
* `help()` lists the built-ins, the script's `def` functions, and the bridge's functions and constants.
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
It's only read, so clear it yourself before the next run. The ESP32 firmware connects the abort flag to Ctrl-C on the serial port.

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

### Connecting hardware (`HardwareBridge`)

Hardware functions come from a `HardwareBridge`. All its methods have defaults, so implement only what you need:

| Method | Purpose |
|--------|---------|
| `call(name, &[i64])` | Integer-only functions: the simplest option |
| `call_args(name, &[Arg])` | Functions that also take strings (`Arg::Int` / `Arg::Str`). The default converts to integers and calls `call` |
| `functions() -> &[HwFn]` | Function table. When it's not empty, unknown names and wrong argument counts are rejected before your code runs, and `help()` lists the entries |
| `constants() -> &[(&str, i64)]` | Named constants scripts can read, such as `OUT` or `LED`. Variables with the same name take precedence |

```rust
use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Interpreter};

const FUNCTIONS: &[HwFn] = &[
    HwFn::new("pin_write", "pin, value", "Set an output pin"),
    HwFn::new("show", "text", "Show text on the LCD"),
];

struct MyBoard;

impl HardwareBridge for MyBoard {
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        match name {
            "pin_write" => { /* set_pin(args[0].int()?, args[1].int()?) */ Ok(None) }
            "show" => { /* lcd_print(args[0].str()?) */ Ok(None) }
            _ => Err(Error::UnknownIdentifier),
        }
    }
    fn functions(&self) -> &[HwFn] { FUNCTIONS }
    fn constants(&self) -> &[(&'static str, i64)] { &[("LED", 2), ("HIGH", 1), ("LOW", 0)] }
}

// let mut vm = Interpreter::with_hardware(&mut uart, &mut MyBoard);
// vm.eval("pin_write(LED, HIGH)\nshow('ready')")
```

Return `Error::ValueError` for bad values (a pin that doesn't exist) and `Error::IoError` when the hardware fails
(an I2C device that doesn't answer).

### Native modules (`import`)

A bridge can also offer **modules**: groups of functions and constants written in Rust that scripts load with `import`,
the way MicroPython's `machine` or `time` work.

```rust
use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Module};

const LED_MODULE: Module = Module {
    name: "led",
    doc: "The onboard LED",
    functions: &[
        HwFn::new("on", "", "LED on"),
        HwFn::with_args("blink", 1, 2, "times, [ms]", "Blink the LED"),
    ],
    constants: &[("PIN", 2)],
};

impl HardwareBridge for MyBoard {
    fn modules(&self) -> &[Module] { &[LED_MODULE] }

    fn call_module(&mut self, module: &str, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        match (module, name) {
            ("led", "on") => { /* ... */ Ok(None) }
            ("led", "blink") => { let ms = args.get(1).map_or(Ok(200), |a| a.int())?; /* ... */ Ok(None) }
            _ => Err(Error::UnknownIdentifier),
        }
    }
}
```

```python
import led                  # binds the module to `led`; without import, `led` is unknown
led.blink(3, 100)
print(led.PIN, led)         # 2 <module 'led'>
from led import blink as b  # functions and constants can be imported by name
b(2)
help(led)                   # lists the module's functions and constants
```

* `call_module` always gets the module's real name, also when the script used `import led as l` or `from led import blink`.
* Argument counts are checked against the module's `HwFn` table before `call_module` runs.
* `import nope` and `from led import nope` raise `Error::ImportError`. `led.nope` raises `Error::AttributeError`.
* Modules and their functions are values: `f = led.on; f()` works, and `help()` lists the modules a bridge offers.
* In the RPN code, `led.blink(3)` becomes `[Ident(led), Int(3), CallAttr { blink, 1 }]`: the module is an ordinary
  value on the stack, so every item still holds at most one string (24 bytes per item).

### Building a REPL

`vm.set_interactive(true)` makes top-level expression statements print their value, like Python's prompt
(`1 + 2` prints `3`, strings print in quotes, `None` prints nothing). `vm.output()` gives access to the `Output`,
so the prompt and echo can go to the same place as `print()`. `eval()` can be called again and again; variables and
functions stay defined. The source passed to `eval()` must stay alive as long as the interpreter, because variables
and functions point into it; the ESP32 REPL keeps every line in a fixed 32 KB arena for that reason.

## Running on real ESP32 hardware

The [`micropython-esp32/`](micropython-esp32) folder holds the firmware. It runs Python scripts on ESP32 (Xtensa) boards,
with a MicroPython-style hardware API: time, GPIO, ADC, PWM, I²C, a watchdog, and drawing on the TTGO's screen.
You pick the board with a **Cargo feature** and the script with **`SCRIPT=`**.

### Supported boards (`FEATURES`)

| Board | Cargo feature | Make command | Python `print()` goes to | Extra functions |
|-------|---------------|--------------|--------------------------|-----------------|
| Any ESP32 board (DevKit, WROOM, WROVER, ...) | *(none, default)* | `make flash` | USB/UART serial monitor | `led_on()`, `led_off()` (GPIO2) |
| LilyGO TTGO T-Display (ST7789 1.14" TFT) | `tft` | `make flash FEATURES=tft` | TFT screen **and** serial monitor | `cls`, `color`, `text`, `rect`, `backlight` |

Both builds share every function in [Hardware API](#hardware-api). Run `help()` from a script to list what your build has.

> [!IMPORTANT]
> On a TTGO T-Display, build with `FEATURES=tft`. The default build doesn't start the display or its backlight, so the screen stays dark.

### Quick start (Docker + Make)

Run these in the `micropython-esp32/` folder. The build runs inside the official `espressif/idf-rust:esp32_latest`
Docker image, so you don't need the Xtensa toolchain on your machine. Flashing and the monitor run on the host with `espflash`.

```bash
cd micropython-esp32

make flash                                   # generic ESP32, runs scripts/demo.py
make flash FEATURES=tft                      # TTGO T-Display, runs scripts/ttgo_demo.py
make flash SCRIPT=scripts/blink.py           # run your own script
make flash FEATURES=tft SCRIPT=scripts/help.py
make flash PORT=/dev/ttyUSB0                 # pick a serial port explicitly

make build                                   # build only
make monitor                                 # serial monitor only (CTRL+R reset, CTRL+C exit)
make clean                                   # remove build artifacts
```

| Make variable | Default | Meaning |
|---------------|---------|---------|
| `FEATURES` | *(empty)* | Cargo features passed as `--features ...` (e.g. `tft`) |
| `SCRIPT` | *(empty: the board's demo)* | Python file to embed, relative to `micropython-esp32/`. It must be inside the repository, because the Docker build only sees the repository |
| `KEEP_MAIN` | *(empty)* | `KEEP_MAIN=1` keeps a `main.py` saved in flash. By default `make flash` erases it, so the new build runs its embedded script |
| `PORT` | *(empty, espflash picks the saved port)* | Serial port for `flash` / `monitor` |

The `SCRIPT` file is compiled into the firmware (`include_str!`). To change scripts without reflashing, use the REPL
or upload them over USB (next section).
Docker builds keep the Cargo registry in `target/cargo-registry`, so crates are downloaded only once.

Without Docker/Make (the `esp` toolchain installed locally with `espup`):

```bash
cargo build --release                                       # generic ESP32
MPY_SCRIPT=scripts/blink.py cargo build --release           # with your own script
cargo build --release --features tft                        # TTGO T-Display
espflash flash --monitor target/xtensa-esp32-none-elf/release/micropython-esp32
```

### REPL and uploading scripts over USB

At boot the board runs `main.py` from flash if one is saved, otherwise the built-in script, and then starts a
**REPL** on the USB serial port (115200 baud), like MicroPython. On the TTGO, the prompt and output also appear on the screen.

```text
$ make monitor                      # or: mpremote connect /dev/ttyACM0 repl, picocom, screen, ...
>>> x = 21
>>> x * 2
42
>>> def blink(n):
...     while n: pin_toggle(LED); sleep_ms(200); n -= 1
...
>>> blink(6)
>>> while True: pass
KeyboardInterrupt                   # after pressing Ctrl-C
>>>
```

| Key | What it does |
|-----|--------------|
| Enter | Runs the line. A line ending in `:` starts a block; an empty line runs the block |
| Backspace, Up arrow | Edit the line; recall the previous line |
| Ctrl-C | Stops the running script, even inside `while True:` or `sleep_ms()`, or clears the line |
| Ctrl-D | Soft reset (on an empty line): clears variables and functions, runs `main.py` again |
| Ctrl-E | Paste mode: paste code with its indentation, Ctrl-D runs it, Ctrl-C cancels |
| Ctrl-A / Ctrl-B | Enter / leave the raw REPL (for tools) |

Every line that runs stays in memory until the next soft reset, because variables and functions point into their
source text. The REPL has 32 KB for that; when it runs out, press Ctrl-D.

**Tools.** The firmware speaks MicroPython's raw REPL protocol, so `mpremote` can run code on it, and
[`tools/mpy.py`](micropython-esp32/tools/mpy.py) adds saving `main.py` to flash:

```bash
tools/mpy.py run scripts/blink.py        # run a file and print its output (no reflashing)
tools/mpy.py exec "print(6 * 7)"
tools/mpy.py save scripts/blink.py       # store as main.py in flash: it runs at every boot
tools/mpy.py cat                         # print the saved main.py
tools/mpy.py erase                       # remove it (the built-in script runs again)
tools/mpy.py reset                       # soft reset

mpremote connect /dev/ttyACM0 run scripts/blink.py
mpremote connect /dev/ttyACM0 exec "print(ticks_ms())"
mpremote connect /dev/ttyACM0 repl
```

`tools/mpy.py` needs `pyserial` and takes `--port` (default `/dev/ttyACM0`, or `$MPY_PORT`). It exits with status 1
when the script raises an error. `mpremote`'s file commands (`cp`, `ls`, `fs`) don't work, because the firmware has no
file system or `os` module; use `tools/mpy.py save` instead. Like with MicroPython, `mpremote` soft-resets the board
before each command unless you add `resume`.

`main.py` is stored in the `nvs` flash partition (24 KB max) with a checksum. If it hangs the board, press Ctrl-C at
boot, or run `tools/mpy.py erase`.

### Example scripts

| Script | Shows |
|--------|-------|
| [`scripts/demo.py`](micropython-esp32/scripts/demo.py) | Default for the generic build: language features, time, ADC, I²C scan, LED blink and PWM dimming |
| [`scripts/ttgo_demo.py`](micropython-esp32/scripts/ttgo_demo.py) | Default for the TTGO: the above plus buttons, battery ADC and a status bar drawn on the screen |
| [`scripts/help.py`](micropython-esp32/scripts/help.py) | `help()`: every function and constant of the build |
| [`scripts/blink.py`](micropython-esp32/scripts/blink.py) | `pin_mode`, `pin_toggle`, `sleep_ms` |
| [`scripts/button.py`](micropython-esp32/scripts/button.py) | Counting button presses with a pull-up and debouncing, timed with `ticks_ms` |
| [`scripts/pwm_fade.py`](micropython-esp32/scripts/pwm_fade.py) | Fading an LED, and a 50 Hz servo pulse |
| [`scripts/i2c_scan.py`](micropython-esp32/scripts/i2c_scan.py) | Scanning the I²C bus and reading a register |
| [`scripts/adc.py`](micropython-esp32/scripts/adc.py) | Averaging ADC samples and converting them to millivolts |
| [`scripts/tft_clock.py`](micropython-esp32/scripts/tft_clock.py) | An uptime clock drawn with `text()` (TTGO only) |
| [`scripts/display_demo.py`](micropython-esp32/scripts/display_demo.py) | `import display`: lines, rectangles, circles, triangles, pixels and text, with timings (TTGO only) |
| [`scripts/calculator.py`](micropython-esp32/scripts/calculator.py) | A two-button calculator app: a keypad on the screen, `buttons` events, `global` state (TTGO only) |

### Calculator app (TTGO T-Display)

[`scripts/calculator.py`](micropython-esp32/scripts/calculator.py) is a complete app in about 190 lines of Python on top
of the `display` and `buttons` modules:

```text
┌──────────────────────────────┐
│ 12 +                     345 │   pending operation (small), current number (big)
├───────┬───────┬───────┬──────┤
│   7   │   8   │   9   │  /   │
│   4   │   5   │   6   │  *   │   the highlighted key is orange
│   1   │   2   │   3   │  -   │
│   C   │   0   │   =   │  +   │
└───────┴───────┴───────┴──────┘
```

| Button | Short press | Hold (600 ms) |
|--------|-------------|---------------|
| LEFT (GPIO0) | Move to the next key | Move to the previous key |
| RIGHT (GPIO35) | Press the highlighted key | Delete the last digit |

It works like a pocket calculator: integers up to 9 digits per entry, operations run left to right (`2 + 3 * 4 =` → 20),
division rounds toward zero, and division by zero or results beyond ±999 999 999 999 show `Error` (C clears it).

```bash
tools/mpy.py run scripts/calculator.py     # try it (Ctrl-C in the terminal to stop the script on the board)
tools/mpy.py save scripts/calculator.py    # make it the app that starts at every boot
tools/mpy.py erase                         # back to the built-in demo
```
| [`scripts/watchdog.py`](micropython-esp32/scripts/watchdog.py) | The watchdog resetting a hung script, and `reset_cause()` after the reboot |
| [`scripts/errors.py`](micropython-esp32/scripts/errors.py) | A forbidden pin reported as `ValueError` with its line number |

### Hardware API

The names follow MicroPython's `time` and `machine` modules, as flat functions (there's no `import` or `Pin` object yet).
Pins are plain GPIO numbers.

| Area | Functions | Notes |
|------|-----------|-------|
| Time | `sleep_ms(ms)`, `sleep_us(us)`, `ticks_ms()`, `ticks_us()`, `ticks_diff(new, old)` | `delay_ms(ms)` is an alias of `sleep_ms`. Ticks count from boot as 64-bit values and don't wrap |
| GPIO | `pin_mode(pin, mode)`, `pin_write(pin, value)`, `pin_read(pin)`, `pin_toggle(pin)` | Modes: `OUT`, `IN`, `IN_PULLUP`, `IN_PULLDOWN`. `pin_write` on an unconfigured pin makes it an output; `pin_read` on one makes it an input. Outputs read back their own level |
| ADC | `adc_read(pin)` | Raw 12-bit value (0–4095, 11 dB attenuation, about 0–3.3 V) on GPIO34, 36 and 39 |
| PWM | `pwm(pin, freq, duty)`, `pwm_stop(pin)` | `duty` in percent (0–100). Up to 4 pins at once, each with its own frequency (LEDC timers 0–3). The duty resolution adapts to the frequency (14 bits at 50 Hz for servos). `pwm_stop` leaves the pin as a low output |
| I²C | `i2c_probe(addr)`, `i2c_scan()`, `i2c_read_reg(addr, reg)`, `i2c_read_reg16(addr, reg)`, `i2c_write_reg(addr, reg, value)` | 100 kHz on SDA = GPIO21, SCL = GPIO22. `i2c_scan()` returns the number of devices; loop over `i2c_probe` to list addresses. 16-bit reads are big-endian. A device that doesn't answer raises `IoError` |
| Watchdog | `wdt_enable(timeout_ms)`, `wdt_feed()`, `wdt_disable()` | The RTC watchdog resets the whole board when it isn't fed in time. `sleep_ms` feeds it automatically every 100 ms |
| System | `reset()`, `reset_cause()` | `reset_cause()` returns `PWRON_RESET`, `HARD_RESET`, `WDT_RESET`, `DEEPSLEEP_RESET` or `SOFT_RESET` (MicroPython's values). The firmware also prints the reset reason at boot |
| TFT (`tft` only) | `cls()`, `color(r, g, b)`, `text(x, y, s)`, `rect(x, y, w, h)`, `backlight(on)` | `color` sets the text and fill color (0–255 per channel). `text` and `rect` draw at pixel positions. `WIDTH` and `HEIGHT` are 240 and 135 |

#### Modules

| Module | Builds | Contents |
|--------|--------|----------|
| `time` | both | `sleep(s)`, `sleep_ms(ms)`, `sleep_us(us)`, `ticks_ms()`, `ticks_us()`, `ticks_diff(new, old)`, with MicroPython's names, so `import time; time.sleep_ms(100)` works as on MicroPython |
| `display` | `tft` | Drawing on the 240 × 135 TFT. See below |
| `buttons` | both | Debounced button events with short and long presses. TTGO: `LEFT` = GPIO0, `RIGHT` = GPIO35; generic build: `LEFT` = BOOT button (GPIO0) |

```python
import display
from display import WIDTH, HEIGHT, rgb

display.fill(display.BLACK)
display.rect(0, 0, WIDTH, HEIGHT)                            # color is optional: WHITE
display.line(0, 0, WIDTH - 1, HEIGHT - 1, display.RED)
display.fill_circle(120, 67, 30, rgb(255, 128, 0))
display.fill_triangle(170, 120, 230, 120, 200, 70, display.MAGENTA)
display.text(8, 120, 'hello', display.YELLOW, display.BLUE)  # text color, background
```

| `display` function | Draws |
|--------------------|-------|
| `fill(color)` | The whole screen. Also moves the console cursor to the top |
| `pixel(x, y, [color])` | One pixel |
| `line(x0, y0, x1, y1, [color])`, `hline(x, y, w, [color])`, `vline(x, y, h, [color])` | Lines |
| `rect(x, y, w, h, [color])`, `fill_rect(...)` | Rectangle outline / filled |
| `circle(x, y, r, [color])`, `fill_circle(...)` | Circle around (x, y) with radius r |
| `triangle(x0, y0, x1, y1, x2, y2, [color])`, `fill_triangle(...)` | Triangle outline / filled |
| `text(x, y, s, [color], [bg])` | Text in the 6 × 10 font, top-left corner at (x, y) |
| `rgb(r, g, b)` | Returns the RGB565 color for 0–255 components |
| `number(x, y, n, [color], [bg])` | Draws an integer (no `str()` needed) and returns its width in pixels |
| `font(size)` | Font for `text` and `number`: 1 = 6 × 10 (default), 2 = 8 × 13, 3 = 10 × 20 pixels |
| `number_width(n)`, `text_width(s)`, `font_height()` | Sizes in the current font, for centering and right-aligning |

```python
display.font(3)
w = display.number_width(total)
display.number(display.WIDTH - 4 - w, 10, total)   # right-aligned big number
```

`buttons` functions:

| Function | Returns |
|----------|---------|
| `wait([timeout_ms])` | The next event, waiting for it; `NONE` after `timeout_ms`. Ctrl-C stops it, and it feeds the watchdog |
| `get()` | The next event, or `NONE` right away |
| `pressed(button)` | 1 while `LEFT` or `RIGHT` is held down |

Events: `LEFT` / `RIGHT` for a short press (sent on release), `LEFT_LONG` / `RIGHT_LONG` once a button has been held
for `LONG_MS` (600 ms, sent while still held; no short event follows). Readings must be stable for 20 ms (debouncing),
and up to 8 events wait in a queue.

```python
import buttons
while True:
    event = buttons.wait()
    if event == buttons.LEFT:
        print('left')
    elif event == buttons.RIGHT_LONG:
        break
```

* Colors are RGB565 integers, like MicroPython's `st7789` drivers. The constants are `BLACK`, `WHITE`, `RED`, `GREEN`,
  `BLUE`, `YELLOW`, `CYAN`, `MAGENTA`, `ORANGE` and `GRAY`, plus `WIDTH` and `HEIGHT`. The color defaults to `WHITE`.
* Shapes may extend past the screen edges; drawing is clipped. Coordinates and sizes beyond ±4096 raise `ValueError`,
  so a typo like `circle(0, 0, 10**9)` can't keep the board busy.
* Timings on the TTGO: full-screen `fill` 22 ms, a full diagonal line under 1 ms, 20 lines from a script loop 132 ms
  (mostly the interpreter loop).
* The console text and the drawing share the screen: `print()` output scrolls over drawings, so call `display.fill()`
  first and draw after printing.
* The older flat functions (`cls`, `color`, `text`, `rect`, `backlight`) still work.

Constants: `OUT`, `IN`, `IN_PULLUP`, `IN_PULLDOWN`, `HIGH`, `LOW` and the `*_RESET` values in both builds;
`LED` (2) and `BOOT_BUTTON` (0) in the generic build; `BUTTON_L` (0), `BUTTON_R` (35), `BAT_ADC` (34) and `ADC_EN` (14) on the TTGO.

**Pins scripts may use.** Anything else raises `ValueError`, which protects the flash chip, the serial console and the pins the firmware owns:

| Build | Allowed GPIOs | Reserved |
|-------|---------------|----------|
| Generic | 0, 2, 4, 5, 12–19, 23, 25–27, 32, 33, 35 | 1/3 (UART0 console), 6–11 (flash), 21/22 (I²C), 34/36/39 (ADC) |
| TTGO T-Display | 0, 2, 12–15, 17, 25–27, 32, 33, 35 | as above, plus 4, 5, 16, 18, 19, 23 (display) |

GPIO34–39 are input-only on the ESP32, so `pin_mode(35, OUT)` raises `ValueError`.

```python
# Blink until the BOOT button is pressed, with a watchdog in case the loop hangs
wdt_enable(2000)
pin_mode(BOOT_BUTTON, IN_PULLUP)
while pin_read(BOOT_BUTTON) == 1:
    pin_toggle(LED)
    sleep_ms(250)          # also feeds the watchdog
wdt_disable()
print('stopped after', ticks_ms(), 'ms')
```

### Implementation details

#### Firmware layout

| File | Purpose |
|------|---------|
| `micropython-esp32/src/bin/main.rs` | Entry point. Each board has a `#[cfg(feature = ...)]` `main()`, a `HardwareBridge` adding its own functions on top of `Board`, and its function and constant tables |
| `micropython-esp32/src/board.rs` | `Board`: time, GPIO, ADC, PWM, I²C, watchdog and reset functions for every build, the `time` module, and the `board_functions!` and `board_constants!` table macros |
| `micropython-esp32/src/serial_console.rs` | `EspSerialOut`, an `Output` that writes `print()` text to USB/UART with `esp_println` |
| `micropython-esp32/src/tft_console.rs` | `TftConsole`: the scrolling text console and drawing primitives on the ST7789 (only compiled with `tft`) |
| `micropython-esp32/src/display_module.rs` | The `display` module: its function table, constants and argument checks (only compiled with `tft`) |
| `micropython-esp32/src/buttons.rs` | The `buttons` module: debouncing, short/long press detection and the event queue |
| `micropython-esp32/src/repl.rs` | The REPL: boot script, line editing, blocks, paste mode, soft reset, and the raw REPL protocol |
| `micropython-esp32/src/serial_input.rs` | UART0 receive interrupt: fills a 2 KB ring buffer and sets the global `ABORT` flag on Ctrl-C |
| `micropython-esp32/src/script_store.rs` | `main.py` in the `nvs` flash partition (`esp-storage`), with a header and checksum |
| `micropython-esp32/tools/mpy.py` | Host tool: run, save, show and erase scripts over USB serial |
| `micropython-esp32/scripts/` | The demo and example scripts |
| `micropython-esp32/build.rs` | Picks the script (`MPY_SCRIPT`, or the board's demo) and hands its path to `include_str!` |
| `micropython-esp32/Cargo.toml` | Declares the `tft` feature. `mipidsi`, `embedded-graphics` and `embedded-hal-bus` are optional dependencies, so boards without a display don't pull them in |
| `micropython-esp32/Makefile` | Docker build and host flash/monitor, with the `FEATURES`, `SCRIPT`, `KEEP_MAIN` and `PORT` variables |

#### How the hardware layer works

* **Pins by number at runtime.** `esp-hal` normally hands out one typed object per pin at startup. To let scripts pick
  pins, `Board` takes them with `AnyPin::steal(n)` (after checking the allowlist) and drives them with `Flex`, which can
  switch between input and output.
* **ADC.** The `esp-hal` ADC driver is typed per pin, so the three ADC pins are set up once at boot. They're the
  input-only GPIO34/36/39, so analog inputs can't collide with digital outputs.
* **PWM.** Each `pwm()` call configures an LEDC timer and channel and then drops the driver objects; the hardware keeps
  running. It tries duty resolutions from 14 bits down until one fits the requested frequency.
* **Watchdog.** `wdt_enable` sets the RTC watchdog's first stage to reset the whole system. `sleep_ms` waits in 100 ms
  chunks and feeds the watchdog in between, so only a script that stops sleeping (a busy loop, a hang) gets reset.
* **TTGO console.** `print()` and the drawing functions share one `TftConsole` through a `RefCell`: `print()` goes
  through `SharedConsole` (the `Output`), drawing through the bridge.
* **Ctrl-C.** A UART0 receive interrupt moves incoming bytes into a ring buffer and sets the global `ABORT` flag when it
  sees Ctrl-C. The interpreter checks that flag before every statement (`set_abort_flag`), and `sleep_ms` checks it
  every 100 ms, so any script can be stopped. Output still goes through `esp_println`, with `\n` sent as `\r\n`.
* **Raw REPL.** Ctrl-A switches to MicroPython's machine protocol: input isn't echoed, Ctrl-D runs it, and the reply is
  `OK`, the output, `\x04`, the error, `\x04`, `>`. Raw-paste mode is declined (`R\x00`), so tools fall back to plain
  raw mode. `tools/mpy.py` sends `#@save`, `#@cat` and `#@erase` through the same channel.

#### Generic ESP32 build (default)

* The onboard LED is on **GPIO2**, common on ESP32 DevKit boards. If yours is elsewhere, change `LED_PIN` in `src/bin/main.rs`,
  or just use `pin_write(n, 1)` in your script.
* `print()` output appears in the serial monitor (`make monitor`).

#### TTGO T-Display build (`--features tft`)

* The display driver is `mipidsi` (ST7789) on SPI2 at 40 MHz in SPI mode 0.
* The screen runs in landscape at **240 × 135 px** (rotated 90°, panel offset 52/40, inverted colors).
* Console text uses `FONT_6X10` in a 6 × 12 px cell with a 4 px margin: **38 columns × 10 rows**, light green on black
  by default (`color()` changes it). The text area covers y = 4 to 123, so y ≥ 124 is free for a status bar.
* `\n` starts a new line, `\r` returns to column 0, and long lines wrap. When the screen is full, the text **scrolls up
  half a screen**. Each line is rendered into a 5.6 KB RAM strip and sent in one SPI transfer. A full redraw takes about
  25 ms, so `help()` (about 40 lines) prints in about 1.4 s.
* Scrolling repaints only the text area. Drawings inside it are overwritten by later `print()` output; call `cls()` first
  for a full-screen drawing.
* All output is copied to the serial monitor as well.

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
3. In `src/bin/main.rs`, add a `#[cfg(feature = "myboard")]` `main()` that creates a `Board` with your pin allowlist, and a
   `HardwareBridge` whose `call_args` handles your extra functions and passes the rest to `board.call(...)`.
   Build its tables with `board_functions!(HwFn::new(...), ...)` and `board_constants!(("NAME", value), ...)`.
4. Add a default demo in `scripts/` and pick it in `build.rs`, then build with `make flash FEATURES=myboard`.

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

Reset reason: Some(ChipPowerOn)
scripts/demo.py (built-in)
=== micropython-rs ===
Hello from ESP32
a = 21 b = 21
(a+b)*3-2**4//2 = 118
square 1 = 1 odd
square 2 = 4 even
square 3 = 9 odd
slept 200 ms
ADC34 raw: 1468
I2C devices: 0
Blinking onboard LED 3 times...
LED dimmed to 10% with PWM

micropython-rs REPL on ESP32
help() lists functions. Ctrl-C stops a script, Ctrl-D soft resets, Ctrl-E pastes.
>>>
```

TTGO T-Display build (`make flash FEATURES=tft`). The same text, the REPL included, is shown on the TFT screen:
```bash
...
I (131) boot: Loaded app from partition at offset 0x10000
Reset reason: Some(ChipPowerOn)
scripts/ttgo_demo.py (built-in)
=== micropython-rs ===
Board: TTGO T-Display
a = 21 b = 21
(a+b)*3-2**4//2 = 118
square 1 = 1 odd
square 2 = 4 even
square 3 = 9 odd
slept 200 ms
ADC34 raw: 1560
I2C devices: 0
buttons (1 = up): 1 1
battery ADC raw: 1565
TFT console active!

micropython-rs REPL on ESP32
help() lists functions. Ctrl-C stops a script, Ctrl-D soft resets, Ctrl-E pastes.
>>>
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
- [x] Hardware API on ESP32: time, GPIO, ADC, PWM, I²C, watchdog, reset cause, TFT drawing; `help()`
- [x] Scripts loaded from `.py` files at build time (`make flash SCRIPT=...`)
- [x] REPL over USB serial with Ctrl-C, paste mode and soft reset; MicroPython raw REPL (`mpremote run/exec`)
- [x] `main.py` saved in flash and uploaded over USB (`tools/mpy.py save`)
- [x] `import` of native modules written in Rust: `time` and a `display` drawing module (pixels, lines, shapes, text)
- [x] `global` statement, `abs`/`min`/`max`; `buttons` module and a two-button calculator app on the TTGO
- [ ] Compact Bytecode Compiler & Stack VM


## Contributing

Contributions are welcome! Whether you are fixing a typo, adding a new Python built-in, or improving docs for fellow Rust learners, feel free to open an issue or submit a pull request.


## License

Licensed under either of Apache License, Version 2.0

