# micropython-rs

![CI status](https://github.com/bieli/micropython-rs/actions/workflows/ci.yml/badge.svg)
![github_tag](https://img.shields.io/github/v/tag/bieli/micropython-rs)
[![Crates.io](https://img.shields.io/crates/v/micropython_rs.svg)](https://crates.io/crates/micropython_rs)
[![docs.rs](https://img.shields.io/docsrs/micropython_rs)](https://docs.rs/micropython_rs)


MicroPython virtual machine written in RUST language for embedded systems


# Intro

A minimal, zero-dependency, `#![no_std]` Python interpreter written in Rust.

Designed from scratch to run on bare-metal microcontrollers (like ARM Cortex-M) **without** needing the Rust standard library (`std`) or a system allocator: all memory is fixed-size and given by your firmware.

---

## Why This Project?

If you are new to Rust or embedded development, this codebase is a clean, readable starting point to learn how interpreters work under the hood:

* **No system allocator:** Fixed-size arrays, string slices (`&str`) of the script source, and one fixed-size heap
  (a byte buffer you provide) with a garbage collector for strings built at runtime. No `Vec`, `String` or `alloc`.
* **Hardware Agnostic (`#![no_std]`):** Connects to any display, UART serial port, or terminal via a simple 1-method `Output` trait.
* **Modular & Easy to Read:** Split into small, self-contained files (`lexer.rs`, `ast_parser.rs`, `interpreter.rs`,
  `strings.rs`, `bytes.rs`, `containers.rs`, `heap.rs`, `flatfs.rs` and `lib.rs`).

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
| Values | `int` (64-bit), `float` (64-bit), `str`, `bytes`, `bool` (`True`/`False`), `None`, `list`, `tuple`, `dict`, `range` |
| Containers | `[1, 2]`, `(1, 2)`, `{'k': v}`, `x[i]`, `x[a:b]`, `x[k] = v`, `del x[k]`, `+`, `*`, `in`, `==`/`<`, comprehensions `[f(x) for x in it if c]` / `{k: v for ...}`, list methods `append extend insert pop remove index count clear reverse sort copy`, dict methods `get pop setdefault update keys values items clear copy` |
| Strings | Escapes (`'\n'`, `'\x41'`, `'\u263a'`), `+`, `*`, `len()`, `s[i]`, `s[a:b]`, `in` / `not in`, `str()`, `repr()`, `int(s, base)`, `ord()`, `chr()`, `s.encode()`, and methods `upper lower strip lstrip rstrip startswith endswith find count replace split join isdigit isalpha isalnum isspace isupper islower` |
| Bytes | `b'...'` literals with escapes, `bytes()`, `bytes(n)`, `bytes(s, 'utf-8')`, `bytes([1, 2])` (any iterable of 0–255), `+`, `*`, `len()`, `b[i]` (an `int`), `b[a:b]`, `in`, `==`/`<`, `hex()`, `decode()`, `startswith endswith find` |
| Modules | built in: `binascii` (`hexlify`, `unhexlify`), `os` (files), `sys` (`path`, `version`, `platform`, `exit()`, ...), `builtins`; plus the native modules of the firmware (e.g. `display`) |
| Literals | `42`, `1_000`, `0x1F`, `0o17`, `0b1010`, `'single'`, `"double"`, `'''triple'''`, adjacent strings joined (`'ab' 'cd'`); lines join inside brackets and after `\` |
| Operators | `+ - * / // % **`, `& \| ^ ~ << >>`, `== != < <= > >=` (chainable), `in`, `not in`, `is`, `is not`, `and or not`, `a if c else b` |
| Assignment | `x = ...`, `a, b = b, a`, `(a, b) = ...`, `a = b = 0`, `x[k] = v`, `+= -= *= /= //= %= **= &= \|= ^= <<= >>=` (also `x[k] += v`), `del a, b`, `del x[k]` |
| Statements | `if`/`elif`/`else`, `while`, `for x in ...` (also `for k, v in d.items()`), `break`, `continue`, `pass`, `def`, `return`; `;` between statements (`x = 1; y = 2`) |
| Exceptions | `try`/`except`/`else`/`finally`, `except (A, B) as e`, `raise`, `raise X('msg')`, bare `raise`, `assert`; built-in classes such as `ValueError`, `KeyError`, `OSError`, `Exception` with CPython's hierarchy; `e.args`, `e.errno` |
| Functions | `def f(a, b=1, *args, **kwargs):`, keyword arguments `f(1, b=2)`, unpacking `f(*items, **options)`, `lambda x: x * 2`, functions as values (`key=len`, `handlers['up']()`), closures and decorators (`@logged`) |
| Classes | `class Dog(Animal):` with `__init__`, attributes (`self.x = v`, `self.n += 1`), methods, class attributes, `@classmethod`, `@staticmethod`, `super()`, `__str__`/`__repr__`, `__enter__`/`__exit__` (`with`), `isinstance`/`type`, your own exception classes (`class SensorError(OSError)`) |
| Calls | `print(..., sep=, end=)`, `abs`, `min`, `max`, `bool`, `len`, `list`, `tuple`, `dict`, `range`, `sorted(key=, reverse=)`, `reversed`, `sum`, `any`, `all`, `enumerate`, `zip`, `format`, `isinstance`, `type`, `hasattr`, `getattr`, `callable`, `hex`, `bin`, `oct`, `id`, `dir`, `globals()`, your own `def` functions, and hardware functions from `HardwareBridge` |
| Formatting | `'%5d mV' % v`, `'%s=%r' % (k, v)`, `'{:>8}'.format(s)`, `'{name}'.format(name=x)`, f-strings `f'{v:04x}'`, with fill, alignment, sign, width, `,` separators and `d x X o b c s r f %` |
| Scope | `global a, b` in a function, to assign globals (also creates them) |
| Modules | `import m`, `import m as x, n`, `from m import f, C as K`; `m.f(...)`, `m.CONST`, for native modules written in Rust and for your own `m.py` files ([Your own modules](#your-own-modules-py-files)) |

Not supported yet: complex numbers, sets, generators (`yield`), `nonlocal`, operator methods on classes (`__add__`, `__eq__`,
`__len__`, ...), multiple inheritance, packages (`import a.b`).
See [ROADMAP.md](ROADMAP.md) for what's planned.

### Arithmetic operators

Integers are 64-bit (`i64`), floats double precision (`f64`, see [Floats](#floats)). Precedence goes from lowest to
highest, as in Python:

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
* `/` always gives a float (`7 / 2` → `3.5`, `6 / 3` → `2.0`), and so does a negative power (`2 ** -1` → `0.5`).
* *Differs:* results that don't fit in `i64` raise `Error::Overflow`. CPython has unlimited-size integers.
* `x / 0`, `x // 0` and `x % 0` raise `Error::ZeroDivision`.
* Strings support `+` and `*` (see [Strings](#strings));
  other operators on strings, and mixing strings with numbers (`'a' + 1`), raise `Error::TypeError`.
* `bool` behaves as an integer (`True + 1` → `2`); `&`, `|`, `^` of two bools give a bool.
* `and`/`or` return one of their operands and skip the right side when the result is known,
  so `0 and f()` never calls `f`.
* Strings compare with each other (`'a' < 'b'`). `==`/`!=` between different types is `False`/`True`;
  ordering them (`1 < 'a'`) raises `Error::TypeError`.
* *Differs:* `<<` that doesn't fit in `i64` raises `Error::Overflow`; a negative shift count raises `Error::ValueError`.

### Floats

Double precision (`f64`), like MicroPython built with `MICROPY_FLOAT_IMPL_DOUBLE` (its ESP32 port uses single
precision by default, so it prints fewer digits). Checked against CPython by
[`tests/cpython/floats.py`](tests/cpython/floats.py), on the PC and on the ESP32.

```python
import math
print(7 / 2, 0.1 + 0.2, 1e16, 2 ** 0.5)       # 3.5 0.30000000000000004 1e+16 1.4142135623730951
print(-7.5 // 2, -7.5 % 2, round(2.5), round(2.675, 2), int(-3.9))   # -4.0 0.5 2 2.67 -3
print('%.2f V, %e, %g' % (3.14159, 1234.5, 1e-7), f'{0.1234:.1%}')   # 3.14 V, 1.234500e+03, 1e-07 12.3%
print(math.sqrt(2), math.floor(-2.5), math.log(8, 2), math.pi)       # 1.4142135623730951 -3 3.0 3.141592653589793
```

| Area | Supported |
|------|-----------|
| Literals | `1.5`, `.5`, `1.`, `1e3`, `2.5e-3`, `1_000.5` |
| Operators | `+ - * / // % **` with ints and floats mixed; `//` and `%` floor like Python (`-7.5 % 2` → `0.5`); comparisons, `==` between `1` and `1.0` (also as dict keys) |
| Built-ins | `float(x)` (also `'inf'`, `'nan'`, `'1_000.5'`), `int(x)` (towards zero), `round(x)` (half to even, an int), `round(x, n)`, `abs`, `min`, `max`, `sum`, `divmod`, `pow(x, y)`, `pow(x, y, m)`, `isinstance(x, float)` |
| `str()` / `repr()` | The shortest text that reads back as the same float, laid out like CPython: `0.1`, `1e+16`, `1.5e-05`, `inf`, `nan`, `-0.0` |
| Formatting | `%f %e %g %F %E %G`, `'{:.2f}'`, `'{:,.2f}'`, `'{:e}'`, `'{:g}'`, `'{:.1%}'`, `'{:.3}'`, f-strings; `'%d' % 3.7` → `'3'` |
| `math` | `pi e tau inf nan`, `sqrt pow exp expm1 log log2 log10`, `sin cos tan asin acos atan atan2`, `sinh cosh tanh asinh acosh atanh`, `floor ceil trunc` (ints), `fabs fmod modf frexp ldexp copysign hypot`, `isfinite isinf isnan`, `degrees radians`, `erf erfc gamma lgamma`, `factorial` |
| Errors | `1 / 0` `ZeroDivisionError`, `2.0 ** 10000` and `math.exp(1000)` `OverflowError`, `math.sqrt(-1)` and `math.log(0)` `ValueError`, `int(float('nan'))` `ValueError` |
| Hardware | Floats reach the `HardwareBridge` as `Arg::Float` (`arg.float()` also takes ints); `time.sleep(0.5)` works on the ESP32. A float where a function wants an int (`pin_write(2, 1.0)`) raises `TypeError`, as in MicroPython |

* *Like MicroPython, differs from CPython:* an int meeting a float is converted to a float first, so ints beyond 2**53
  compare and divide like their nearest float; `sum()` adds floats one by one (CPython 3.12 compensates rounding:
  `sum([0.1] * 10)` is `1.0` there, `0.9999999999999999` here and in CPython 3.11).
* *Differs:* a negative number to a fractional power (`(-8) ** (1/3)`) raises `ValueError`: CPython gives a complex
  number, which isn't supported.
* The math functions come from [`libm`](https://crates.io/crates/libm) (musl's, in Rust); results may differ from
  CPython's C library in the last binary digit for the transcendental ones (`sin`, `exp`, ...).
* Floats are part of the `medium` and `full` [feature levels](#feature-levels) (Cargo feature `float`). Without them,
  float literals and `/` raise `NotImplementedError`, as in MicroPython built without floats. On the ESP32 they add
  86 KB of flash (math functions 24 KB, Rust's float formatting and parsing 14 KB plus tables, the interpreter's
  float code 8 KB) and no RAM. A float addition in a loop is about 20 % slower than an int one; int-only loops got
  about 2 % slower.

#### Single precision (`float32`)

The Cargo feature `float32` (`make flash FLOAT=32` for the firmware) makes floats `f32`, like MicroPython's ESP32
port: 7 significant digits are printed (`1 / 3` → `0.3333333`, `0.1 + 0.2` → `0.3`), and results are rounded to
single precision (`16777217.0` → `16777216`, `1e39` → `inf`). The `math` functions use libm's `f32` versions
(`sinf`, `sqrtf`, ...). Floats still reach the `HardwareBridge` as `f64` (exactly).

Speed on the ESP32 (TTGO, 80 MHz): its FPU only does single precision, so `f64` runs in the ROM's software
routines (the `f64` firmware calls them from 377 places; the `f32` one has 670 FPU instructions instead).

Measured on a TTGO T-Display, release build, `CPU_MHZ=80`, the same board for both builds. *Native*: 10 000
rounds of the operation in Rust, without the interpreter (a temporary benchmark function in the firmware, not part
of it). *Python*: a 500-pass `for` loop with one line, time per pass, including the loop itself.

| Operation | `f64` (default) | `f32` (`FLOAT=32`) | `f32` vs `f64` |
|-----------|----------------:|-------------------:|---------------:|
| Native `a = a * 3 + 5` (`i64`, for comparison) | 264 ns | 264 ns | |
| Native `a = a * 1.0001 + 0.5` | 1924 ns | 175 ns | 11× faster |
| Native `a = 10 / (a + 1)` | 8302 ns | 2383 ns | 3.5× faster |
| Native `sqrt(x)` | 6148 ns | 2000 ns | 3.1× faster |
| Native `sin(x)` | 32 225 ns | 18 200 ns | 1.8× faster |
| Python `for i in range(500): pass` | 74 µs | 74 µs | the same |
| Python `x = (x * 3 + i) % 65536` (ints) | 236 µs | 247 µs | (build noise) |
| Python `y = y * 1.0001 + i * 0.5` | 258 µs | 254 µs | 2 % faster |
| Python `y = i / 7.3` | 183 µs | 183 µs | the same |
| Python `z = math.sin(i * 0.01)` | 406 µs | 352 µs | 13 % faster |
| Python `z = math.sqrt(i)` | 270 µs | 239 µs | 11 % faster |
| Python `s = '%.3f' % (i / 7)` | 571 µs | 597 µs | the same (formatting uses the `f64` value) |
| Python `v = adc_read(34) * 3.3 / 4095` and `c = (v - 0.5) * 100.0` | 617 µs | 681 µs | (build noise, ADC reads) |
| App image (TTGO build) | 424 KB | 418 KB | 6 KB smaller |
| REPL stack frame (holds the interpreter) | 22 640 B | 22 640 B | the same |

So the maths itself is 2-11 times faster (an `f32` multiply-add is even faster than an `i64` one), but a script
barely notices: running one line of Python costs 70-250 µs in this interpreter (it parses as it runs), against
about 2 µs for a software `f64` operation. Only scripts heavy on `math` functions gain about 10 %. Differences
within about ±5-10 % depend on the build (code placement in flash and IRAM): the int-only loop, which has no floats
at all, varied that much between the two firmwares. `f32` uses no less RAM: values are as large as the 64-bit ints
either way. The bigger speed-ups are `make flash CPU_MHZ=240` (3× for everything, "CPU clock" below) and the
planned bytecode compiler; `float32` is for MicroPython's behaviour (its digits and rounding), and 6 KB of flash.

How it works without a heap: the parser turns each expression into postfix (RPN) order in a fixed-size buffer.
For example, `(a + 2) * -3` becomes `[a, 2, Add, 3, Neg, Mul]`.
The interpreter then evaluates it with a small fixed-size value stack.
The limits are in `src/ast_parser.rs`:

| Constant | Value | Meaning |
|----------|-------|---------|
| `MAX_EXPR_ITEMS` | 64 | Operands and operators per statement, all call arguments and list elements included |
| `MAX_TARGETS` | 6 | Names in `a, b = ...` and `for a, b in ...` |
| `MAX_EXPR_DEPTH` | 16 | Nesting of parentheses and unary operators |
| `MAX_CALL_ARGS` | 16 | Arguments per function call (positional and keyword) |
| `MAX_PARAMS` | 8 | Parameters per `def` or `lambda` |

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
* Parameters can have default values (`def blink(times, ms=250):`), evaluated once when `def` runs, as in Python.
  Calls can name arguments: `blink(3, ms=100)`. A missing, unknown or repeated argument raises `Error::TypeError`.
* Functions are values: `on_press = blink`, `sorted(names, key=len)`, `actions = {'up': up, 'down': down}`, and
  `lambda` makes small ones: `sorted(points, key=lambda p: p[1])`. Up to 32 `def` functions, methods and lambdas
  exist at a time (`MAX_FUNCS`); a lambda or method takes one slot however often it's made, and the slots of ones
  nothing refers to any more are reclaimed by the garbage collector.
* `*args` collects extra positional arguments into a tuple, `**kwargs` extra keyword arguments into a dict, and
  `f(*items, **options)` unpacks them in a call.
* A function or lambda made inside a function is a **closure**: it remembers the values the enclosing function's
  variables had when it was made (Python remembers the variables themselves; for decorators and callbacks it's the
  same), so decorators work:

```python
def logged(fn):
    def wrapper(*args, **kwargs):
        result = fn(*args, **kwargs)
        print(fn.__name__, args, '->', result)
        return result
    return wrapper

@logged
def area(w, h=1):
    return w * h

area(3, h=4)                                  # area (3,) -> 12
```

```python
v = 1234
print('%5d mV' % v, '{:>8}|'.format('right'), f'{v:,} mV', f'0x{v:04X}')   #  1234 mV    right| 1,234 mV 0x04D2
print('a', 'b', sep='-', end='!\n')                                         # a-b!
```
* `break`/`continue` outside a loop and `return` outside a function raise `Error::InvalidSyntax`,
  even in code that never runs, as Python does at compile time.
* Bad indentation raises `Error::IndentationError`.

How it works without a heap: the interpreter runs the source while parsing it and never builds a program in memory.
A `while` loop keeps a copy of the parser (96 bytes) from its header and jumps back to it after each pass.
A `def` stores a parser copy that points at its body. A block that doesn't run is still parsed, so syntax errors are always reported.
The trade-off is speed: loop bodies are parsed again on every pass. The planned bytecode compiler will remove that cost.

Limits (in `src/interpreter.rs`), for the `full` [feature level](#feature-levels) (`minimal` / `medium` in brackets):

| Constant | Value | Meaning | Error |
|----------|-------|---------|-------|
| `MAX_VARS` | 128 (48 / 96) | Variables: globals plus the locals of all active calls | `OutOfMemory` |
| `MAX_FUNCS` | 96 (16 / 32) | `def` functions, methods and lambdas (the firmware's `machine` module takes 48, `network` 11) | `OutOfMemory` |
| `MAX_GLOBAL_DECLS` | 16 (8 / 16) | Names declared `global` by all active calls together | `OutOfMemory` |
| `MAX_CALL_DEPTH` | 8 (6 / 8) | Nested function calls, recursion included | `RecursionLimit` |
| `MAX_NESTING` | 40 (24 / 40) | Nested blocks (`if`/`while`/function bodies) running at once | `RecursionLimit` |

Native stack use grows with each nested call (an RPN buffer and parser state per call, about 7.8 KB on the ESP32,
see [Stack budget](#stack-budget-measured-on-the-esp32)), which is why `MAX_CALL_DEPTH` is small. On a 64-bit host the `Interpreter` struct is about 45 KB (including the 128 variable
slots, the 128-value stack, the 512-entry object table of the heap and the 96-slot function table); on the ESP32 it's about 30 KB.

### Strings

```python
name = 'TTGO'
msg = 'Hello, ' + name + '!\t\x41'           # escapes are decoded
print(msg, len(msg), repr(msg))              # Hello, TTGO!	A 14 'Hello, TTGO!\tA'
print(msg.upper(), msg[7:11], msg[-3:], 'GO' in msg, msg.find('!'))
print('v=' + str(adc_vref()) + ' mV', int('0x1F', 16), chr(9786), ord('A'))
```

* Indices and lengths count characters (Unicode code points), like Python: `len('héllo')` is 5.
* `repr()` returns a valid Python literal (quotes and escapes chosen like CPython), which is also what the REPL echoes.
* `str(s)` returns `s` itself (strings never change); a missing index raises `Error::IndexError`, `int('x')` raises
  `Error::ValueError`.
* All test expressions are checked against CPython. Formatting (`%`, `format()`, f-strings) is described under
  [Control flow and functions](#control-flow-and-functions); a method can be taken without calling it
  (`up = s.upper`, `w = f.write`).

### Bytes

```python
import binascii
data = b'\x00\x10\xfe' + 'zażółć'.encode()
print(data, len(data), data[2], data.hex())   # b'\x00\x10\xfeza\xc5\xbc...' 13 254 0010fe7a61...
print(binascii.unhexlify('48656c6c6f').decode(), binascii.hexlify(b'OK'))   # Hello b'4f4b'
```

`print()`, `str()` and `repr()` of bytes all give the `b'...'` literal, like Python. Bytes and strings never mix:
`b'a' + 'a'` raises `Error::TypeError` and `b'a' == 'a'` is `False`. Invalid hex digits and undecodable UTF-8 raise
`Error::ValueError` (CPython raises `binascii.Error` and `UnicodeDecodeError`, both subclasses of `ValueError`).
Not supported yet: `bytearray`, and the text methods (`upper`, `split`...) on bytes.

### Exceptions

```python
def read_sensor(pin):
    if pin not in (32, 33, 34, 35, 36, 39):
        raise ValueError('not an ADC1 pin: ' + str(pin))
    return adc_read(pin)

try:
    mv = read_sensor(5)
except ValueError as e:
    print('config error:', e)                   # config error: not an ADC1 pin: 5
except OSError:
    print('sensor failed')
finally:
    led_off()                                   # runs in every case
```

* Errors of the interpreter are Python exceptions: `1 // 0` raises `ZeroDivisionError`, `{}['k']` a `KeyError`, a
  failing flash file operation an `OSError`, and so on, with CPython's class hierarchy (`except LookupError` catches
  both `KeyError` and `IndexError`; `except Exception` catches all of them).
* An uncaught exception prints a Python traceback on the board (and in Thonny / mpremote):
  `Traceback (most recent call last):`, `File "<stdin>", line 4, in <module>`, `ValueError: not an ADC1 pin: 5`.
* Can't be caught: syntax errors (like in Python, where they happen before the code runs) and the step limit.
  **Ctrl-C** raises `KeyboardInterrupt`, which `except KeyboardInterrupt:` catches, but a bare `except:` doesn't,
  unlike CPython: a `while True: try: ... except: pass` loop can still be stopped.
* Not supported yet: your own exception classes (they need classes), exception chaining (`raise X from Y` ignores
  `Y`), and tracebacks through function calls (the line is where the exception happened).

### Classes

```python
class Sensor:
    count = 0                                   # class attribute

    def __init__(self, pin, name='sensor'):
        self.pin = pin
        self.name = name
        Sensor.count += 1

    def read(self):
        return adc_mv(self.pin)

    def __str__(self):
        return '%s on pin %d' % (self.name, self.pin)

    @classmethod
    def made(cls):
        return cls.count

class Battery(Sensor):
    def __init__(self):
        super().__init__(35, 'battery')

    def read(self):
        return super().read() * 2               # a voltage divider halves it

class SensorError(OSError):                     # your own exceptions
    pass

bat = Battery()
print(bat, bat.read(), Sensor.made(), isinstance(bat, Sensor))   # battery on pin 35 4120 1 True
```

* A class is a heap object with a dict of its attributes; an instance has a dict of its own. Single inheritance:
  attributes are looked up in the instance, then the class and its bases. `super()` works in methods.
* `__init__`, `__str__`, `__repr__` (also inside lists: `print([p1, p2])`), `__enter__`/`__exit__` (`with obj as x:`)
  are called by the interpreter; other special methods (`__add__`, `__eq__`, `__len__`, `__getitem__`, ...) aren't
  (yet).
* Classes are defined at the top level (not inside a function). An exception class can't have `__init__`: its
  arguments are stored as given (`e.args`).
* The body of a `class` statement runs like a function call, so it can contain any statement: `import os` inside a
  class makes `cls.os`, as Thonny's helper class uses it.
* Checked against CPython like the rest (`tests/cpython/classes.py`, `closures.py`).

### Lists, tuples and dicts

```python
readings = [adc_read(36) for _ in range(8)]          # a list comprehension
print(sum(readings) // len(readings), max(readings), sorted(readings)[:3])

pins = {'led': 2, 'button': 0}
pins['buzzer'] = 25
for name, pin in pins.items():
    print(name, pin)

counts = {}
for word in 'the cat the dog'.split():
    counts[word] = counts.get(word, 0) + 1
print(counts)                                        # {'the': 2, 'cat': 1, 'dog': 1}
x, y = 10, 20
x, y = y, x
```

* Lists, tuples, dicts and ranges are objects on the heap (see Memory below). `b = a` shares the list, as in Python:
  `b.append(1)` changes `a` too. A list grows as needed; a dict keeps insertion order and finds keys by comparing
  them one by one (fine for the small dicts of a microcontroller).
* Dict keys can be numbers, strings, bytes, `None`, bools and tuples of those; a list or dict as a key raises
  `Error::TypeError`, a missing key `Error::KeyError`.
* Differences from CPython, to stay small: `d.keys()`, `d.values()`, `d.items()`, `enumerate()`, `zip()` and
  `reversed()` return lists, and a generator expression (`sum(x for x in a)`) is evaluated as a list. A
  comprehension has one `for` (and optionally one `if`). Slices have no step (`a[::2]`). Containers nested more than
  8 levels deep are printed as `[...]`.
* All behaviour above (results and error kinds) is checked against CPython.

**Memory.** A literal without escapes, and any slice of it, points into the script source and costs no memory.
Strings, bytes, lists, tuples and dicts created at runtime (`+`, `*`, `str()`, escapes, `upper()`, `hexlify()`,
`[...]`, `{...}`, ...) live in a fixed-size **heap**: a byte buffer the application passes to `Interpreter::with_heap`.
When it's full, a **mark-and-sweep garbage collector** keeps everything reachable from variables and from the value
stack (the operands of every expression being evaluated, including those of calling functions, and the iterables of
running `for` loops), and everything inside reachable lists and dicts. It frees the rest (cycles included) and moves
the survivors together. Values refer to heap objects through handles, so moving them is safe. Limits: the buffer size
and the object table: the heap's own has 512 entries at the `full` level, 256 at `minimal` and `medium`
(`heap::MAX_OBJECTS`; every string inside a list counts, and the frozen modules keep about 250: `machine` 160,
`network` 65); `Interpreter::with_heap_table(&mut [HeapEntry::FREE; N])` gives it any size up to 65 535 (12 bytes an
entry; one per 32-64 bytes of heap suits most scripts). Beyond either, `Error::OutOfMemory`.

The collector is built for big heaps too: its work list is 64 entries on the stack (containers beyond that are marked
"deferred" in the table and picked up afterwards), it looks only at the table entries below the highest one in use,
and compaction orders the survivors by a radix sort of (offset, index) pairs in the heap's free memory (16 bytes a
survivor; when that's not free, an in-place heapsort): no memory proportional to the table on the stack, and
O(n log n) or better.
`gc.mem_free()` and `gc.objects_free()` show how much of each is left, `gc.collect()` frees garbage at once.
Without a heap, creating strings or containers raises `Error::OutOfMemory`; everything else works.

```rust
let mut heap = [0u8; 16 * 1024];
let mut vm = Interpreter::new(&mut uart).with_heap(&mut heap);
vm.eval("print('a' + 'b' * 3)")?;                     // abbb
let (used, capacity, objects, collections) = vm.heap_stats();
```

The ESP32 firmware gives the interpreter a 32 KB heap (static memory, not the stack). On the TTGO, a loop creating
2 000 strings in it takes 1.6 s, with the collector running as needed. Everything in this section is also checked on
the board: the scripts in `tests/cpython/` print exactly what CPython prints when they run on the TTGO
(`tools/mpy.py run tests/cpython/classes.py`, ...).

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
        Some(line) => print!("Error at line {}: ", line),
        None => print!("Error: "),
    }
    vm.write_exception(err, &mut uart);    // `ZeroDivisionError: division by zero`, `ValueError: bad`
    println!();
}
```

`Error` says what went wrong (`Error::ZeroDivision`, ...). A script's own `raise` returns `Error::Exception`;
`write_exception` prints its class and message, the way Python's traceback ends (`print_exception` writes to the
interpreter's own `Output`).

Syntax errors point at the offending token, and runtime errors at the statement that failed.
`write_traceback(err, "<stdin>", &mut uart)` prints the whole traceback as MicroPython does (the firmware's REPL,
the host REPL and `mpy-cli` use it). An error in an imported module's code names that module's file and line, under
the script's line that led there; `error_file()` and `error_caller_line()` give the same in code:

```text
Traceback (most recent call last):
  File "<stdin>", line 3, in <module>
  File "sensors.py", line 5, in <module>
ZeroDivisionError: division by zero
```

A statement too big for the parser's fixed buffers (64 RPN items, 16 levels of nesting) is a
`SyntaxError: expression too complex: split it into smaller statements` (`Error::TooComplex`) at the statement's
first line. A `MemoryError` can be caught even when the heap is completely full: the interpreter keeps one
exception object for it, made while there was room (as MicroPython does).

Two guards stop a script that runs too long, such as an accidental `while True:`:

```rust
use core::sync::atomic::{AtomicBool, Ordering};

static ABORT: AtomicBool = AtomicBool::new(false);   // set to true from a UART/button interrupt

vm.set_step_limit(Some(10_000));  // eval() returns Error::StepLimit after 10 000 statements/loop passes
vm.set_abort_flag(&ABORT);        // eval() returns Error::Interrupted once ABORT is true
```

The step budget starts over for each `eval()` call. The abort flag is checked before every statement and loop pass.
It's only read, so clear it yourself before the next run. The ESP32 firmware connects the abort flag to Ctrl-C on the serial port.

A third guard protects the native stack, which on a microcontroller is small and crashes the device when it
overflows:

```rust
// The stack grows down: below this address, scripts get a RecursionError
vm.set_stack_floor(Some(stack_bottom_address + 16 * 1024));
```

The interpreter compares the stack position (the address of a local variable) with the floor at every call, block
and expression, so deep recursion or deeply nested code raises `RecursionError` (`Error::RecursionLimit`), which a
script can catch. The margin is for what runs below the last check: hardware drivers, flash writes, interrupts. The
ESP32 firmware takes the bottom from esp-hal's linker script (`_stack_end_cpu0`) and keeps 16 KB.

#### Stack budget (measured on the ESP32)

Measured on a TTGO T-Display (release build, `FEATURES=tft`) with
[`scripts/stack_budget.py`](micropython-esp32/scripts/stack_budget.py). The firmware fills the unused stack with a
pattern at boot; `stack_peak()` returns the deepest point since its previous call and fills it again. The figures
include everything below the script: `main`, the REPL and `eval()`.

| Code | Peak stack |
|------|-----------:|
| A statement at the top level (`x = 1`) | 51.5 KB |
| Each further Python call (`def` function or method) | + 7.5 KB |
| Each nested block (`if`, `for`, `while`, `try`, `with`) | + 1.6 KB |
| Recursion until `RecursionError` (`MAX_CALL_DEPTH` = 8 calls) | 121.5 KB |
| Methods with `with`, `try`, `*args`/`**kwargs`, f-strings, comprehensions, until `RecursionError` | 149 KB |
| **Worst case found**: 4 nested blocks per call, until `RecursionError` (`MAX_NESTING` = 40) | **157 KB** |

The table above was measured before the function table moved out of the `Interpreter` struct (see below). The
main stack is what the 520 KB of RAM leaves after statics, the REPL's buffers in `dram2` and esp-hal; the
interpreter's limits stop scripts above the stack guard's floor (16 KB margin): the guard is the second line of
defense (with Wi-Fi it's the one that stops deep recursion).

The function table and the statement cache are statics of the firmware (`Interpreter::with_func_slots`,
`with_stmt_cache`), not part of the `Interpreter` struct: the struct is 15 KB on the ESP32 instead of 30, and starting
the REPL, which builds, moves and keeps it, needs about 40 KB of stack instead of 88 (`repl::run` 16 KB,
`new_interpreter` 15 KB, `from_parts` 8.6 KB). Measured on the TTGO with Wi-Fi (`FEATURES=tft,wifi`): a top-level
statement takes 37.6 KB of the 83.5 KB stack (before: 53.6 of 102 KB). A call with positional arguments only (most
calls) copies them from the value stack instead of building `CallArgs` (room for 16 positional and 16 keyword
arguments, 768 bytes) in a frame of its own: a Python call takes 5.5 KB of stack instead of 7.5, and 5 nested calls
fit next to the Wi-Fi driver (4 before); function calls got faster too (4.4 -> 5.0 thousand a second at 80 MHz).
Without Wi-Fi the stack is 162 KB. `make report` lists the biggest stack frames of a build.

#### Statement cache

The interpreter runs source text: without help, every pass of a loop lexes and parses each statement of its body
again, which was over half of a loop's time (callgrind on the PC: the lexer and parser took 50-70 % of the
instructions, the arithmetic under 2 %). `Interpreter::with_stmt_cache(&mut [StmtCache::EMPTY; N])` keeps up to `N`
parsed statements (direct-mapped by their place in the source; statements of up to 16 RPN items), so a loop's or a
function's body is parsed once. Each entry is about 0.5 KB on a 32-bit chip (1 KB on a 64-bit PC); the cache is
cleared by every `eval` (the REPL reuses its source memory). The firmware gives it 16 entries (8 on the ESP32 with
Wi-Fi: in main RAM, paid for by a 4 KB smaller Wi-Fi heap, 68 KB, of which the driver used at most 53 KB with the
station, the access point, scans and NTP at once); `mpy-cli` and the host REPL 64. On the PC a 300 000-pass loop
went from 0.31 to 0.11 s; on the ESP32 at 80 MHz (`machine.bench()`, thousands per second):

| Python test | Without the cache | With it |
|-------------|------------------:|--------:|
| int math `(x*3+i)%k` | 3.9 | 8.1 |
| float math `y*a+b` | 4.0 | 8.7 |
| function call | 3.4 | 4.5 |
| list append | 4.0 | 5.7 |
| dict set | 2.7 | 3.5 |
| `pin_write` | 1.9 | 2.5 |
| empty `for` loop | 11.0 | 11.5 |

The interpreter's tests run every script with and without a (3-entry) cache and compare the output, errors and
error lines.

On another MCU, budget about 52 KB for `eval()` plus 8 KB per call level you allow, or use a smaller [feature
level](#feature-levels) or lower `MAX_CALL_DEPTH` and `MAX_NESTING` and set the stack floor. Keep the `Interpreter`
(30 KB on 32-bit targets with the `full` level) in one place: build it
with `new()`/`with_hardware()` and the `set_` methods rather than moving it through the `with_` builders.

**No `unsafe` code.** The core library is `#![forbid(unsafe_code)]` (except the `link_section` attribute of the
`iram` feature, which the lint counts as unsafe; every other build proves the rest). The firmware's `unsafe` is in
the board code: re-acquiring peripherals, the stack painting and the static buffers.

**Speed on the ESP32 (`iram` feature).** The firmware enables the core library's `iram` feature, which puts the
interpreter's hot loop (lexer, parser, statement and expression evaluation, about 40 KB) into the `.rwtext` section,
i.e. the ESP32's internal instruction RAM. Running from flash, the loop missed the 32 KB flash cache all the time:
5000 empty `while` passes took 1.6 s instead of 0.9 s.

**CPU clock (`CPU_MHZ`).** The firmware runs the ESP32 at 80 MHz unless built with `CPU_MHZ=160` or `CPU_MHZ=240`
(esp-hal sets the clock once, at boot, so it can't change while running like MicroPython's `machine.freq(f)`).
Measured on the TTGO:

| `CPU_MHZ` | 5000 empty loops | 2000 arithmetic loops | `pin_write` | `led.value(1)` (`machine`) |
|----------:|-----------------:|----------------------:|------------:|---------------------------:|
| 80 (default) | 902 ms | 677 ms | 265 µs | 1165 µs |
| 160 | 451 ms | 339 ms | 145 µs | 600 µs |
| 240 | 301 ms | 226 ms | 120 µs | 425 µs |

The buses (UART, I²C, SPI) and PWM run from the 80 MHz peripheral clock, and `sleep_ms` and the ticks from a timer,
so they're the same at every clock (checked at 240 MHz: baud rates, loopbacks, `sleep_ms(1000)` = 1000.4 ms). A
faster clock draws more current, which matters on a battery.

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
    let mut heap = [0u8; 8 * 1024];  // optional: memory for strings built at runtime
    let mut vm = Interpreter::new(&mut uart).with_heap(&mut heap);
    let _ = vm.eval("print('Hello from bare metal!')");
}
```

### Feature levels

The core library comes in three levels, chosen with Cargo features. `full` is the default; each level includes the
one before:

| Level | Python | Limits (functions / variables / call depth) |
|-------|--------|---------------------------------------------|
| `minimal` | ints, bools, `None`, strings, bytes, lists, tuples, dicts, comprehensions, `if`/`while`/`for`, `def` with defaults and keyword arguments, `lambda`, closures, decorators, exceptions, native modules and hardware functions, `builtins` | 16 / 48 / 6 |
| `medium` | + floats and `math`, classes (`class`, `super()`, `@classmethod`, `__str__`, `__call__`, your own exceptions), string formatting (`%`, `str.format`, f-strings, `format()`, `hex()`/`bin()`/`oct()`) | 32 / 96 / 8 |
| `full` | + files (`open()`, `os`), importing `.py` files and frozen modules, `sys`, `binascii`, `gc`: everything Thonny and mpremote need | 96 / 128 / 8 |

```toml
micropython-rs = { path = "...", default-features = false, features = ["medium"] }
# or only the parts you need:
micropython-rs = { path = "...", default-features = false, features = ["minimal", "format"] }
```

The parts are `classes`, `format`, `files`, `modules` and `stdlib`. Without a part, its code isn't compiled
in (a small stub takes its place), and using it raises `NotImplementedError: not enabled in this build`;
`import os` / `import sys` raise `ImportError`. The API stays the same at every level (`with_files`,
`with_frozen_modules` and so on just have nothing to do), so an application builds against any of them.

The ESP32 firmware (`make flash LEVEL=minimal`), measured on the generic build and a TTGO T-Display:

| `LEVEL` | Interpreter code | App image | Interpreter in RAM | Stack for a top-level statement | Recursion until `RecursionError` |
|---------|-----------------:|----------:|-------------------:|--------------------------------:|--------------------------------:|
| `minimal` | 142.0 KB | 258 KB | 10.0 KB | 29 KB | 85 KB (6 calls) |
| `medium` | 218.8 KB (floats: about 60 KB of it) | 356 KB | 15.5 KB | 35 KB | 108 KB (8 calls) |
| `full` (default) | 231.2 KB + 13 KB frozen `machine.py`, `time.py` | 380 KB | 31.4 KB (96 function slots, 512 objects; before the function table moved out: 16 KB now) | 51.5 KB | 121.5 KB (8 calls) |

"Interpreter in RAM" is the REPL's stack frame, which holds the `Interpreter` struct; its tables shrink with the
level. Speed is the same at every level. Thonny, mpremote's file commands and the `machine` module need `full`.

### Connecting hardware (`HardwareBridge`)

Hardware functions come from a `HardwareBridge`. All its methods have defaults, so implement only what you need:

| Method | Purpose |
|--------|---------|
| `call(name, &[i64])` | Integer-only functions: the simplest option |
| `call_args(name, &[Arg])` | Functions that also take strings or bytes (`Arg::Int` / `Arg::Str` / `Arg::Bytes`; `arg.bytes()` accepts both). The default converts to integers and calls `call` |
| `call_bytes(name, &[Arg], out: &mut [u8]) -> usize` | Functions that **return bytes** (marked `.returning_bytes()` in the table): write into `out` (up to `MAX_BRIDGE_BYTES` = 512, less when the heap is fuller) and return the length. The script gets a `bytes` object. `call_module_bytes` is the same for modules |
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

```rust
const BUS: &[HwFn] = &[
    HwFn::with_args("bus_write", 2, 2, "addr, data", "Send bytes"),
    HwFn::with_args("bus_read", 2, 2, "addr, n", "Read n bytes").returning_bytes(),
];

impl HardwareBridge for MyBus {
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        match name {
            "bus_write" => { let data: &[u8] = args[1].bytes()?; /* send(args[0].int()?, data) */ Ok(Some(data.len() as i64)) }
            _ => Err(Error::UnknownIdentifier),
        }
    }
    fn call_bytes(&mut self, name: &str, args: &[Arg<'_>], out: &mut [u8]) -> Result<usize, Error> {
        match name {
            "bus_read" => { let n = (args[1].int()? as usize).min(out.len()); /* receive(&mut out[..n]) */ Ok(n) }
            _ => Err(Error::UnknownIdentifier),
        }
    }
    fn functions(&self) -> &[HwFn] { BUS }
}
```

### Rich return values (floats, strings, tuples, lists, dicts)

`call` and `call_module` return an `i64`, `call_bytes` a `bytes`. A function marked `HwFn::returning_values()` can
return anything instead: the interpreter calls `HardwareBridge::call_values` with a `Returns` to fill, and builds the
Python objects from it. Nothing recorded is `None`; several values at the top level are a tuple.

```rust
const SENSOR: Module = Module {
    name: "sensor",
    doc: "A BME280-style sensor",
    functions: &[HwFn::new("read", "", "(°C, hPa, %)").returning_values()],
    constants: &[],
};

impl HardwareBridge for MyBoard {
    fn call_values(&mut self, module: Option<&str>, name: &str, args: &[Arg<'_>],
                   out: &mut Returns<'_>) -> Result<(), Error> {
        match (module, name) {
            (Some("sensor"), "read") => {
                let (t, p, h) = self.bme.read();   // -> sensor.read() == (21.37, 1013.2, 45.8)
                out.begin_tuple()?;
                out.float(t)?;
                out.float(p)?;
                out.float(h)?;
                out.end()
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }
    // ...
}
```

`Returns` has `none`, `bool`, `int`, `float` (in the build's precision), `str`, `bytes` (or `value(Ret::...)`), and
`begin_tuple` / `begin_list` / `begin_dict` ... `end` (keys and values alternate in a dict), nesting up to 8 deep.
It records the values in a scratch area at the top of the heap, above every object the arguments point to: string
arguments stay valid while the function runs, and nothing is allocated meanwhile. The interpreter then builds the
objects (long lists in chunks: a list of 300 tuples works with the 128-value stack). If the values don't fit the
heap's free memory (64 KB at most), the script gets a `MemoryError` it can catch; an unended container is a
`ValueError`, a list as a dict key a `TypeError`. `tests/returns.rs` shows all of it, and the next section is a
complete sensor module built on it.

### A sensor module in Rust (BME280 example)

[`examples/bme280.rs`](examples/bme280.rs) is a complete native module: a driver for Bosch's BME280 (temperature,
pressure and humidity over I2C) that scripts use with `import bme280`. It uses every kind of rich return value. On a
PC a simulated chip answers, loaded with the calibration and readings of the datasheet's worked example:

```
$ cargo run --example bme280
BME280 at 0x76 found at [118]
25.08 C  1006.53 hPa  43.9 %RH
dew point 11.9 C
['humidity', 'pressure', 'temperature'] True
raw (519888, 415148, 28000) T1 27504
no sensor at 0x77
```

```python
import bme280
t, p, h = bme280.read()     # (25.08, 1006.5325390625, 43.8564453125): °C, hPa, %RH as floats (a tuple)
bme280.read_dict()          # {'temperature': 25.08, 'pressure': 1006.5325390625, 'humidity': 43.8564453125}
bme280.info()               # {'chip': 'BME280', 'id': 96, 'address': 118, 'calibration': {'T1': 27504, ...}}
bme280.scan()               # [118]: the addresses (0x76, 0x77) where a BME280 answers (a list)
bme280.raw()                # (519888, 415148, 28000): the ADC values
bme280.init(0x77)           # the sensor at another address; OSError if nothing answers there
```

The file has four parts, in the order to copy them:

1. **The driver** (`Bme280`), written against a two-method `I2c` trait (`write_read`, `write`). It checks the chip ID,
   reads the calibration (including the two 12-bit humidity values that share register 0xE5), and measures in forced
   mode. It uses the datasheet's integer formulas, so it's exact without an FPU and in a `float32` build.
2. **The module table** (`BME280_MODULE`). Functions that return values are marked `.returning_values()`. `init`
   returns nothing, so it stays on `call_module`.
3. **The bridge** (`Board`). `call_values` turns a reading into Python values: `out.begin_tuple()`,
   `out.float(...)` three times, then `out.end()`. `info()` shows a dict inside a dict, and `scan()` a list. A missing
   sensor is `Error::IoError` (an `OSError` in the script) and a bad address is `Error::ValueError`.
4. **The simulated chip** (`SimulatedBme280`): a register file that answers at one address, for tests and the PC.

The driver math stays in Rust, and the script works with ready values; the dew point above is computed in Python from
them. `tests/bme280_example.rs` runs the example's script. It checks the driver against the datasheet's worked example
(25.08 °C, 100653.27 Pa), and against the datasheet's separate floating-point formulas over a range of readings
(−30…+60 °C, 300…1100 hPa, 0…100 %RH).

On a board, implement `I2c` with the HAL's bus (esp-hal's blocking `I2c`, as the firmware's `i2c_*` functions use),
add `BME280_MODULE` to the bridge's `modules()`, and handle `("bme280", ...)` in `call_module` / `call_values`:

```rust
use esp_hal::{i2c::master::I2c as HalI2c, Blocking};

struct Bus<'d>(HalI2c<'d, Blocking>);

impl I2c for Bus<'_> {
    fn write_read(&mut self, addr: u8, out: &[u8], into: &mut [u8]) -> Result<(), Nack> {
        self.0.write_read(addr, out, into).map_err(|_| Nack)
    }
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack> {
        self.0.write(addr, data).map_err(|_| Nack)
    }
}
```

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

### Your own modules (`.py` files)

When no native module has the name, `import sensors` runs `sensors.py` from the file system (`with_files`), like
MicroPython. Give the interpreter memory for module source code with `with_module_memory(&mut buf)`: the text is read
there once and stays (variables and functions point into it), so the buffer only fills up; the ESP32 firmware has 16 KB
and empties it at every soft reset.

```python
# sensors.py
LIMIT = 100
count = 0
def record(value):
    global count          # the module's `count`, not the main script's
    count += 1
    return 'high' if value > LIMIT else 'ok'
```

```python
import sensors
from sensors import record
print(record(150), sensors.count, sensors)   # high 1 <module 'sensors'>
sensors.LIMIT = 200                          # module attributes can be changed
```

* Each module has its own globals (a dict): its functions and classes see the module's names, not the main script's.
* A module runs once: later imports (also from other modules) give the same object.
* A missing file raises `ImportError`; a module that doesn't fit in the module memory raises `MemoryError`. An
  error while the module runs propagates, and the module isn't kept, so a fixed file can be imported again.

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

The REPL itself (the `>>>` prompt, MicroPython's raw REPL that `mpremote`, Thonny and `tools/mpy.py` use, and the
`#@` file commands) is in [`micropython-port/`](micropython-port), a crate without any chip code. A board implements its
`Port` trait (how bytes reach its console and leave it, the lowest stack address scripts may use, its name and
build) and hands over the memory for the sessions, its frozen modules and a flash file system. The ESP32 firmware is
one such board ([`src/repl.rs`](micropython-esp32/src/repl.rs)); `micropython-port/tests/repl.rs` drives the same REPL
on a PC through a mock board.

### Supported boards (`FEATURES`)

| Board | Cargo feature | Make command | Python `print()` goes to | Extra functions |
|-------|---------------|--------------|--------------------------|-----------------|
| Any ESP32 board (DevKit, WROOM, WROVER, ...) | *(none, default)* | `make flash` | USB/UART serial monitor | `led_on()`, `led_off()` (GPIO2) |
| LilyGO TTGO T-Display (ST7789 1.14" TFT) | `tft` | `make flash FEATURES=tft` | TFT screen **and** serial monitor | `cls`, `color`, `text`, `rect`, `backlight` |
| ESP32-S3 boards, e.g. Seeed XIAO ESP32-S3 | `esp32s3` (chip) | `make flash CHIP=esp32s3` | the chip's own USB port (USB-Serial/JTAG) | `led_on()`, `led_off()` (GPIO21, the XIAO's user LED, lit when low) |
| ESP32-C3 boards (RISC-V), e.g. ESP32-C3 Super Mini | `esp32c3` (chip) | `make flash CHIP=esp32c3` | the chip's own USB port (USB-Serial/JTAG) | `led_on()`, `led_off()` (GPIO8, the Super Mini's blue LED, lit when low) |

All builds share every function in [Hardware API](#hardware-api). Run `help()` from a script to list what your build has.
`FEATURES=wifi` and `LEVEL=...` work on every board (`make flash CHIP=esp32s3 FEATURES=wifi`).

#### Seeed XIAO MG24 (Silicon Labs EFR32MG24, Arm Cortex-M33): in progress

The first board outside the ESP32 family, in [`micropython-mg24/`](micropython-mg24): its own firmware on
`micropython-port`, built with stable Rust (no Docker) and flashed with [probe-rs](https://probe.rs) through the
board's own CMSIS-DAP debugger on its USB-C port (no BOOT button):

```bash
rustup target add thumbv8m.main-none-eabihf
cargo install probe-rs-tools --locked
cd micropython-mg24
make flash                     # build, flash, reset (LEVEL=..., FLOAT=64 as for the ESP32 boards)
make size                      # flash and RAM use
../micropython-esp32/tools/mpy.py run myscript.py   # the same tools as the ESP32 boards (mpy.py, mpremote)
```

What works now: the REPL and raw REPL on the USB serial port (USART0 at 115200, bytes received by interrupt,
Ctrl-C stops a script), 32-bit floats (the FPU's precision; `FLOAT=64` computes in software), a 64 KB heap and
about 115 KB of stack (a stack overflow faults, MSPLIM, instead of overwriting memory). The chip runs at its reset
clock, 19 MHz. Next: `machine` and `time` (pins, the LED on PA7, I²C, clock speed), files in flash, ADC/PWM/SPI,
and the Sense's motion sensor. Not planned: Bluetooth, Thread and Zigbee (they need Silicon Labs' closed C stacks).

#### ESP32-S3 and ESP32-C3 (`CHIP=esp32s3`, `CHIP=esp32c3`)

The chip is a Cargo feature (`esp32`, the default, `esp32s3` or `esp32c3`) and a build target
(`xtensa-esp32s3-none-elf`; the C3 is RISC-V: `riscv32imc-unknown-none-elf`, linked by rust-lld); the Makefile picks
both from `CHIP`. What differs ([`src/chip.rs`](micropython-esp32/src/chip.rs)):

| | ESP32 | ESP32-S3 (pins as on the XIAO ESP32-S3) | ESP32-C3 (pins as on the ESP32-C3 Super Mini) |
|-|-------|-----------------------------------------|-----------------------------------------------|
| CPU | Xtensa, 2 cores, FPU | Xtensa, 2 cores, FPU | RISC-V, 1 core, no FPU (floats in software), 80 or 160 MHz |
| Console | UART0 through the board's USB-to-serial chip | The chip's own USB port (USB-Serial/JTAG); no other chip needed | The same |
| GPIO | 0-39; 34-39 input-only | 0-21, 26-48, all can be outputs; 19/20 are USB, 26-37 flash and PSRAM | 0-21; 11-17 flash, 18/19 USB |
| ADC (`adc_read`, `adc_mv`, `machine.ADC`) | GPIO34, 36, 39 | GPIO1, 2, 3 (the XIAO's A0-A2), calibrated by esp-hal from the chip's eFuse values | GPIO0, 1, 3, calibrated the same way; full scale about 2.5 V |
| I2C (default pins) | SDA 21, SCL 22 | SDA 5, SCL 6 (the XIAO's D4/D5) | SDA 6, SCL 7 (8 is the LED, 9 the BOOT button) |
| Script pins | see `GENERIC_PINS` | 0, 4, 7-18, 21, 38-48 (the XIAO's D3 = 4, D6/D7 = 43/44, D8-D10 = 7-9) | 2, 4, 5, 8 (LED), 9 (BOOT, the `buttons` LEFT), 10, 20, 21 |
| SPI (`spi_init`) | SPI3 | SPI3 | SPI2 (its only one for scripts) |
| Main stack | 181 KB, 102 KB with Wi-Fi | 195 KB, 141 KB with Wi-Fi (the driver's heap goes in `dram2`) | 178 KB, 115 KB with Wi-Fi |
| App image | 380 KB, 882 KB with Wi-Fi | 396 KB, 808 KB with Wi-Fi | 417 KB, 851 KB with Wi-Fi |

The C3's core has no atomic instructions: the firmware's read-modify-write atomics go through `portable-atomic`, which
esp-hal sets up for a single core. `make report` reads RISC-V stack frames too.

Measured on an ESP32-C3 Super Mini (`CHIP=esp32c3 FEATURES=wifi`, 80 MHz): flashed and ran at the first try, with no
buttons (it comes up as `303a:1001`); the REPL, `tools/mpy.py`, the LED, pins, pull-ups, `Pin.irq()` (5 edges, 5
callbacks), `machine.Timer`, I2C, ADC; Wi-Fi with its small on-board antenna: 14 networks in 0.74 s, connected in 4.7
s, NTP in 59 ms, the access point next to the station. Speed: a 20 000-pass loop in 3.3 s, the same as the S3 (1.5 s with
`CPU_MHZ=160`, the C3's top speed); 2000 float operations in 0.44 s (software floats). Stack with Wi-Fi: 42.8 KB for a top-level statement, 6.6 KB per call
(RISC-V frames are smaller), the worst case found 109 KB of the 115 KB.

The first flash of a board that runs other firmware (Arduino, CircuitPython) needs its bootloader: hold BOOT (the
XIAO's B button), press and release RESET (R), keep BOOT down a second more, then release it. The chip then shows up
as `303a:1001` (`lsusb`) and `make flash CHIP=esp32s3` works. If it then prints `waiting for download`, press R once:
the reset after that first flash can land in the bootloader again. From then on espflash resets it over USB by
itself. The XIAO has no antenna on the board: snap the one from its box onto the U.FL connector, or Wi-Fi hears
almost nothing (scans find no networks, its own access point is weak).

Measured on a Seeed XIAO ESP32-S3 (`CHIP=esp32s3 FEATURES=wifi`, 80 MHz): the REPL, `tools/mpy.py` (exec, run,
files), Ctrl-C, the LED, pins with pull-ups and pull-downs, `machine.Timer` (a 50 ms timer fires 20 times a second),
the I2C bus, the ADC pins; Wi-Fi: a scan found 6 networks in 0.9 s, connected with DHCP in 4.5 s, NTP in 58 ms, the
access point next to the station. Stack: a top-level statement takes 43.5 KB, recursion stops at `MAX_CALL_DEPTH` (8
calls, 113 KB) before the stack guard, and the worst case found takes 130 KB of the 141 KB.

Wi-Fi and the ADC: the radio's calibration uses the same SAR ADC as `adc_read`/`adc_mv`/`machine.ADC` and changes
its settings (the controller, attenuations, and on the S3 the calibration code, which esp-hal only writes when it
changes). So with Wi-Fi running, each ADC call sets ADC1 up again first, as ESP-IDF does before every one-shot reading:
0.12 ms more per call on the S3 at 80 MHz (0.41 instead of 0.28 ms for one sample, 1.26 instead of 1.14 ms for 64).
Builds without Wi-Fi are unchanged.

The S3's USB-Serial/JTAG port ignores a flush while its previous packet is still going out, so a short write right
after another one would wait for the next write (the raw REPL's closing `\x04>`, and `mpy.py` timed out). The
firmware flushes once more while it waits for input ([`serial_input.rs`](micropython-esp32/src/serial_input.rs)).

**PSRAM** (`make flash CHIP=esp32s3 FEATURES=wifi,psram`): the interpreter's heap and its object table go into the
external RAM (esp-hal maps it at boot; `PSRAM_MODE` is `octal` for the S3, as on the XIAO's 8 MB module, `quad`
otherwise, e.g. an ESP32 WROVER with `FEATURES=psram`). Without PSRAM on the board, the internal 32 KB heap is used.
`machine.lscpu()` shows the `PSRAM size`. On the XIAO ESP32-S3 (80 MHz): a **7.25 MB heap** with 65 535 object
entries (instead of 32 KB and 512); 10 000 rows of `[i, 'row %d' % i, {'v': i * 2}]` (40 000 objects) in 2.1 s, a 1 MB
string and a 2 MB `bytes` object; scripts run as fast as with the internal heap (string formatting twice as fast: the
heap rarely fills). A collection costs about 17 µs per live object (PSRAM is slower than internal RAM): 6 ms with 500,
81 ms with 5000, 0.34 s with 20 000, 1 s with 40 000 live objects; `CPU_MHZ=240` shortens it.

Not there yet on the S3: its second core, and the TTGO's display build.

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
make flash FEATURES=tft CPU_MHZ=240          # 3x faster scripts (more current)
make flash LEVEL=minimal                     # smallest interpreter: no classes, formatting or files

make build                                   # build only
make report                                  # memory of the last build: regions, stack, biggest frames
make monitor                                 # serial monitor only (CTRL+R reset, CTRL+C exit)
make erase-files                             # delete all files on the board (keeps the firmware)
make clean                                   # remove build artifacts
```

| Make variable | Default | Meaning |
|---------------|---------|---------|
| `FEATURES` | *(empty)* | Cargo features passed as `--features ...`: `tft` (the TTGO's display), `wifi` (the [`network` module](#the-network-module-wi-fi)), e.g. `FEATURES=tft,wifi` |
| `LEVEL` | `full` | The interpreter's [feature level](#feature-levels): `full` (everything: Thonny, `machine`), `medium` (classes, formatting) or `minimal` (smallest) |
| `FLOAT` | `64` | Float precision: `64` (double, like CPython) or `32` (single, like MicroPython's ESP32 port; computed by the FPU, see [Single precision](#single-precision-float32)) |
| `CPU_MHZ` | `80` | CPU clock: `80` (least current, for batteries), `160` or `240` (scripts run 2× / 3× faster). Fixed in the firmware, `machine.freq()` reports it |
| `SCRIPT` | *(empty: the board's demo)* | Python file to embed, relative to `micropython-esp32/`. It must be inside the repository, because the Docker build only sees the repository |
| `ERASE_FILES` | *(empty)* | Files on the board survive `make flash`. `ERASE_FILES=1` deletes them all (then the embedded `SCRIPT` runs at boot, as no `main.py` is left); `make erase-files` does the same without flashing |
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
Traceback (most recent call last):  # after pressing Ctrl-C
  File "<stdin>", line 1, in <module>
KeyboardInterrupt
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
tools/mpy.py watch scripts/blink.py      # rerun the file on the board every time you save it
tools/mpy.py watch scripts/blink.py --save   # ... and keep each version as main.py
tools/mpy.py run scripts/blink.py        # run a file and print its output (no reflashing)
tools/mpy.py exec "print(6 * 7)"
tools/mpy.py save scripts/blink.py       # store as main.py in flash: it runs at every boot
tools/mpy.py cat                         # print the saved main.py
tools/mpy.py erase                       # remove it (the built-in script runs again)
tools/mpy.py reset                       # soft reset
tools/mpy.py ls                          # files on the board (see "Files on the board")
tools/mpy.py put lib.py                  # copy a file to the board
tools/mpy.py get data.txt                # copy a file from the board
tools/mpy.py rm old.py                   # delete a file on the board

mpremote connect /dev/ttyACM0 run scripts/blink.py
mpremote connect /dev/ttyACM0 exec "print(ticks_ms())"
mpremote connect /dev/ttyACM0 repl
```

**Is the board's firmware up to date?** The firmware records the git commit it's built from (`make` takes it
from git; `-dirty` when tracked files had uncommitted changes). The banner shows it
(`micropython-rs 0.1.0 (e3f938c) REPL on ESP32`), as do `machine.lscpu()['Commit']` and the raw REPL's `#@version`.
Before every command `tools/mpy.py` compares it with the checkout. When the firmware is older (or so old that it
doesn't report a version), it warns and prints the `make flash` command for the board's chip, level and features:

```
mpy.py: warning: the board's firmware is from commit 1988909, 1 commit(s) older than this
checkout (e3f938c), with changes to the firmware since (e.g. micropython-esp32/src/wifi.rs).
  Flash the current firmware, from micropython-esp32/:
    make flash FEATURES=tft,wifi
```

Commits that only change docs, tests, examples, `tools/` or `scripts/` don't count. The command runs anyway, and
`--no-check` skips the comparison.

**Edit-and-run workflow (`watch`).** Open the file in any editor (VS Code, Thonny as an editor, nano, ...) and run
`tools/mpy.py watch myapp.py` in a terminal. Every time the file is saved:

1. CPython checks the syntax first (the board runs a subset of Python, so anything CPython rejects would fail there
   too); a syntax error is shown with its line number and the previous version keeps running.
2. The running program is stopped (Ctrl-C, works for endless loops too), the board is soft-reset, and the new
   version starts. Its output appears live; runtime errors show the board's line number.
3. With `--save`, each version is also stored as `main.py`; when you stop watching (Ctrl-C), the board soft-resets
   and keeps running it on its own.

```text
$ tools/mpy.py watch myapp.py
watching myapp.py - save it to rerun it on the board, Ctrl-C to stop

--- 21:33:46 running myapp.py ---
version 1, tick 0
version 1, tick 1
--- stopped ---

--- 21:33:49 running myapp.py ---
version 2, tick 100
myapp.py:2: syntax error: unexpected EOF while parsing - not uploaded
--- 21:33:55 running myapp.py ---
--- ZeroDivisionError: division by zero (line 2) ---
```

**Thonny 3.2.7** (the version in Ubuntu 20.04's apt) works in its own MicroPython mode: *Tools → Options →
Interpreter → MicroPython (generic)*, then pick the board's port. The file browser shows the board's files
(*View → Files*), files open from and save to the board (*File → Save as → MicroPython device*), *Run* (F5) runs the
editor's code on the board, *Stop* interrupts it, the shell and the variables view work. See
[Thonny support](#thonny-support) for how this is tested. **Thonny 4.x and 5.x** (from thonny.org, and pip) work the
same way: *Tools → Options → Interpreter → MicroPython (generic)*.

`tools/mpy.py` needs `pyserial` and takes `--port` (default `/dev/ttyACM0`, or `$MPY_PORT`). It exits with status 1
when the script raises an error. `mpremote`'s file commands work too (`mpremote fs ls`, `fs cp main.py :main.py`,
`fs cat`, `fs rm`; there are no directories, so `fs mkdir` fails). Like with MicroPython, `mpremote` soft-resets the
board before each command unless you add `resume`.

If `main.py` hangs the board, press Ctrl-C at boot, or run `tools/mpy.py erase`.

### Files on the board

The firmware has a small flat file system (no directories) in the `vfs` flash partition, defined in
[`micropython-esp32/partitions.csv`](micropython-esp32/partitions.csv): a 3 MB app partition plus 960 KB for files,
so the same layout fits ESP32 boards with 4 MB or more flash. `main.py` lives there and runs at boot.

```bash
tools/mpy.py ls                          # list files
tools/mpy.py put scripts/calculator.py   # copy a file to the board (as calculator.py)
tools/mpy.py put notes.txt data.txt      # ... under another name
tools/mpy.py get data.txt                # copy it back to the PC
tools/mpy.py rm data.txt                 # delete it
tools/mpy.py save scripts/voltmeter.py   # = put as main.py
```

```text
$ tools/mpy.py ls
main.py 1512
calculator.py 4780
# 2 of 30 files, max 32704 bytes each
```

| Property | Value |
|----------|-------|
| Files | Up to 30, flat (no directories), names of 1–40 characters from `A-Z a-z 0-9 . _ -` |
| File size | Up to 32 704 bytes each (one 32 KB slot per file) |
| Crash safety | A new version is written to a free slot and committed last; the old one is dropped only after that. A reset or power cut while saving keeps the previous version (tested by resetting the board in the middle of an upload) |
| Wear | Each save goes to the next free slot, so rewriting `main.py` spreads over the whole partition |
| Integrity | Every file has a checksum; a damaged file reads as `OSError: file corrupted` |
| Transfer | Hex-encoded in 2 KB chunks (binary-safe): about 4 KB/s up and 5 KB/s down at 115 200 baud |
| Errors | `OSError: no such file`, `file too large`, `bad file name`, `no free file slot` |

* Saving needs one free slot, even to replace a file, because the new version is written before the old one is
  dropped. With 30 files, delete one first.
* Files survive `make flash`; `make flash ERASE_FILES=1` or `make erase-files` deletes them all.
* A `main.py` saved by older firmware (in the `nvs` partition) is moved into the file system at the first boot.
* If the firmware is flashed without `partitions.csv` (no `vfs` partition), it says so at boot and runs without files.
* Scripts use the same files with `open()` and the `os` module (below).

**Files in Python scripts:**

```python
import os
with open('log.txt', 'a') as f:                 # 'r', 'w', 'a', plus 'b' for bytes
    f.write('%d mV\n' % adc_mv(36))
with open('log.txt') as f:
    for line in f:                              # also f.read([n]), f.readline(), f.readlines()
        print(line.strip())
print(os.listdir(), os.stat('log.txt')[6], os.size('log.txt'))
os.rename('log.txt', 'old.txt')
os.remove('old.txt')
```

| Function | What it does |
|----------|--------------|
| `open(name, mode='r')` | A file object: `read([n])`, `readline()`, `readlines()`, `write(s)` (returns the count), `close()`, `flush()`, `tell()`, `seek(pos)`, `for line in f`, `with`; attributes `name`, `closed`, `mode` |
| `os.listdir([path])` | All file names (`path` can only be `''`, `'/'` or `'.'`: there are no directories) |
| `os.remove(name)`, `os.rename(old, new)` | Delete / rename a file |
| `os.stat(name)` | `(mode, 0, 0, 0, 0, 0, size, 0, 0, 0)`, like MicroPython |
| `os.size(name)` | The size in bytes (micro:bit style; Thonny uses it) |
| `os.statvfs('/')` | `(bsize, frsize, blocks, bfree, bavail, ...)` with one block per 32 KB file slot |
| `os.uname()`, `os.sync()` | System name tuple; `sync` does nothing (files are saved when closed) |
| `os.getcwd()`, `os.chdir(path)` | `''` (no directories), and `chdir` accepts only `''` / `'/'` |
| `os.mkdir()`, `os.rmdir()` | `OSError` (`EPERM`): there are no directories |

* Text mode (`'r'`, `'w'`) reads and writes `str` (UTF-8); binary mode (`'rb'`, `'wb'`) `bytes`. Reads go
  straight from flash into a new string, so a whole 32 KB file can be read only if the heap has room for it; read
  big files in parts (`f.read(512)`) or line by line.
* Writing streams to flash; the new version replaces the old one when the file is closed (crash safe, see above).
  **One file can be open for writing at a time**: opening a second one saves and closes the first. A file that is
  never closed isn't saved, so use `with`.
* Errors are `OSError` with the usual numbers and names: `[Errno 2] ENOENT: no such file` (`e.errno == 2`), 22 for a bad name, 27
  for a file over 32 KB, 28 when all 30 slots are taken.
* On a PC, `mpy-cli` gives scripts a file system in RAM with the same rules (it's gone when the script ends).
* Embedding: `Interpreter::with_files(&RefCell<dyn FileSystem>)`; `flatfs::FlatFs` implements `FileSystem` for any
  `Flash`, and `flatfs::RamFlash` keeps one in RAM.

The file system itself is hardware-independent code in the core library
([`src/flatfs.rs`](src/flatfs.rs)): any board provides a `Flash` implementation (read, write into erased flash,
erase 4 KB sectors). Its tests run on the PC against a simulated NOR flash, including a power cut at every
possible write during a replace.

#### Thonny support

Thonny reads and saves device files by running Python helper code on the board through the raw REPL. Thonny 3.2.7
(Ubuntu 20.04) uses strings with escapes, `bytes` and `binascii` (files travel hex-encoded), dict comprehensions,
`print(..., sep=, end=)`, `repr()`, `globals()`, `try`/`except`, `del` and `open()`. The flat file system matches
Thonny's "device without directories" mode: `os.getcwd()` returns `''`, which is how Thonny (3.2.7 as well as 4.x
and 5.x) detects it, and then it lists files with `os.listdir()` and `os.size()`. Thonny 4.x/5.x first load a helper
class onto the board (imports and `try`/`except` in the class body, `@builtins.classmethod` methods calling
`cls.builtins.print(...)`), and use `sys.path`.

**How it's tested.** `tools/thonny_e2e.py` runs Thonny's *own* backend code (no GUI) against the interpreter:
`mpy-cli --repl` speaks the same REPL protocol as the firmware, and a pseudo-terminal connects it to Thonny's serial
connection. It checks connecting, the file browser, saving and opening a text file and a binary file with every byte
value, the shell (statements, expressions, tracebacks), *Run*, *Stop* on an endless loop, *Stop/Restart* (soft
reboot), the variables view, the storage info and deleting. `tools/thonny5_e2e.py` does the same with the backend of
Thonny 4.x/5.x (tested with 4.1.7 and 5.0.0), and `tools/mpremote_e2e.py` with mpremote. They all run in CI:

```bash
pip install pyserial jedi asttokens mpremote
pip download --no-deps thonny==3.2.7 -d /tmp/thonny
python3 -m zipfile -e /tmp/thonny/thonny-3.2.7-py3-none-any.whl /tmp/thonny/src
pip download --no-deps thonny==5.0.0 -d /tmp/thonny5
python3 -m zipfile -e /tmp/thonny5/thonny-5.0.0-py3-none-any.whl /tmp/thonny5/src
cargo build
python3 tools/thonny_e2e.py /tmp/thonny/src             # add --port /dev/ttyACM0 to test a real board
python3 tools/thonny5_e2e.py /tmp/thonny5/src
python3 tools/mpremote_e2e.py
```

`python3 tools/pty_repl.py` prints a port (`/dev/pts/N`) with the interpreter on it, to try Thonny, mpremote or a
terminal against it on a PC without a board.

### Example scripts

| Script | Shows |
|--------|-------|
| [`scripts/demo.py`](micropython-esp32/scripts/demo.py) | Default for the generic build: language features, time, ADC, I²C scan, LED blink and PWM dimming |
| [`scripts/ttgo_demo.py`](micropython-esp32/scripts/ttgo_demo.py) | Default for the TTGO: the above plus buttons, battery ADC and a status bar drawn on the screen |
| [`scripts/help.py`](micropython-esp32/scripts/help.py) | `help()`: every function and constant of the build |
| [`scripts/blink.py`](micropython-esp32/scripts/blink.py) | `pin_mode`, `pin_toggle`, `sleep_ms` |
| [`scripts/button.py`](micropython-esp32/scripts/button.py) | Counting button presses with a pull-up and debouncing, timed with `ticks_ms` |
| [`scripts/pwm_fade.py`](micropython-esp32/scripts/pwm_fade.py) | Fading an LED, and a 50 Hz servo pulse |
| [`scripts/i2c_scan.py`](micropython-esp32/scripts/i2c_scan.py) | Scanning the I²C bus, and reading registers with `i2c_write_read` |
| [`scripts/yard_lights.py`](micropython-esp32/scripts/yard_lights.py) | Yard lights: an LED strip on at sunset and off at sunrise, for months. Sunrise/sunset computed from the location, the clock from NTP, EU summer time, a watchdog, a status screen. See [Yard lights app](#yard-lights-app) (`FEATURES=tft,wifi`) |
| [`scripts/wifi_scanner.py`](micropython-esp32/scripts/wifi_scanner.py) | A Wi-Fi scanner app in the calculator's style: networks strongest first with signal bars, dBm, channel and security; a details page with signal quality and BSSID; two-button navigation (`FEATURES=tft,wifi`) |
| [`scripts/wifi_ap.py`](micropython-esp32/scripts/wifi_ap.py) | `network.WLAN(network.AP_IF)`: the board's own Wi-Fi network, showing devices as they join and leave (`FEATURES=wifi`) |
| [`scripts/wifi_connect.py`](micropython-esp32/scripts/wifi_connect.py) | `network.WLAN`: scan, connect with error handling, `ifconfig()` (`FEATURES=wifi`) |
| [`scripts/machine_demo.py`](micropython-esp32/scripts/machine_demo.py) | `from machine import Pin, ADC, PWM, I2C, UART, SPI, WDT`: the `machine` module, checked with two jumper wires |
| [`scripts/stack_budget.py`](micropython-esp32/scripts/stack_budget.py) | How much native stack calls, blocks and recursion take (`stack_peak()`) |
| [`scripts/bus_loopback.py`](micropython-esp32/scripts/bus_loopback.py) | UART and SPI checked with one jumper wire each (TX to RX, MOSI to MISO) |
| [`scripts/adc.py`](micropython-esp32/scripts/adc.py) | Averaging ADC samples and converting them to millivolts |
| [`scripts/tft_clock.py`](micropython-esp32/scripts/tft_clock.py) | An uptime clock drawn with `text()` (TTGO only) |
| [`scripts/display_demo.py`](micropython-esp32/scripts/display_demo.py) | `import display`: lines, rectangles, circles, triangles, pixels and text, with timings (TTGO only) |
| [`scripts/calculator.py`](micropython-esp32/scripts/calculator.py) | A two-button calculator app: a keypad on the screen, `buttons` events, `global` state (TTGO only) |
| [`scripts/voltmeter.py`](micropython-esp32/scripts/voltmeter.py) | A voltmeter: calibrated mV readout, min/max/average, sweeping graph, battery gauge (TTGO only) |

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

### Voltmeter app (TTGO T-Display)

[`scripts/voltmeter.py`](micropython-esp32/scripts/voltmeter.py) turns the board into a small measuring instrument:

```text
┌──────────────────────────────────────┐
│ AIN GPIO36 HOLD [####] 87%  01:23:45 │  input, HOLD flag, battery level, uptime
│ 1234 mV                    3.95 V    │  input in mV (big), battery voltage
│ min 1201  max 1340  avg 1288         │  statistics since the last reset
│3300┬─────────────────────────────────│
│    │      ╱╲      ╱╲                 │  sweeping graph, 20 points per second,
│1650┤ ────╱──╲────╱──╲──── │          │  grey cursor = where the next point goes
│    │          ╲╱      ╲╱             │
│   0┴─────────────────────────────────│
└──────────────────────────────────────┘
```

| Button | Short press | Hold (600 ms) |
|--------|-------------|---------------|
| LEFT (GPIO0) | Switch input: GPIO36 ↔ GPIO39 | Reset min / max / average |
| RIGHT (GPIO35) | HOLD: freeze the readings | Graph range: 3300 → 1650 → 825 mV full scale |

* Each point is the average of 16 readings (`adc_mv(pin, 16)`); the average shown is a moving average.
* The uptime (top right, `HH:MM:SS`, hours go past 99) counts from power-on, not from when the script started, so
  starting from a full battery it shows how long the battery has lasted. It restarts on power-on, a reset button press
  or a watchdog reset; a soft reset (Ctrl-D, `tools/mpy.py`) keeps it running.
* Below about 150 mV the ESP32 ADC reads 0, shown as `< 150 mV`; at the top of its range it saturates, shown as `> 3.1 V`.
* The battery level is linear from 3.3 V (0 %) to 4.2 V (100 %) for a Li-ion cell. Above 4.25 V it shows `CHG`
  (on USB, a charging cell reads about 4.4–4.5 V). Below 3.0 V no usable cell is connected, so the gauge shows a grey
  icon, `USB` and `no battery` instead of a misleading 0 %.
* To match a multimeter, measure one known voltage and set `CAL_NUM` / `CAL_DEN` (gain) and `CAL_OFFSET` (mV) at the
  top of the script.

> [!WARNING]
> ADC pins take 0–3.3 V only. Measure higher voltages through a resistor divider, and never connect more than 3.3 V
> (or a negative voltage) directly to GPIO36/39.

```bash
tools/mpy.py run scripts/voltmeter.py      # try it
tools/mpy.py save scripts/voltmeter.py     # start it at every boot
```

#### ADC accuracy

`adc_mv()` uses ESP-IDF's linear "Vref" characterization for ADC1 at 11 dB attenuation
(`mV = (vref * 196602 / 4096 * raw + 32768) / 65536 + 142`), with the reference voltage the factory stored in eFuse.
ESP-IDF adds a lookup table for the top of the 11 dB range, which isn't included here, so readings above about
2.6 V are a few percent less accurate. Useful range: about 0.15–3.1 V. Averaging (`samples`) reduces the noise.

The firmware also works around an `esp-hal` 1.0 bug: on the ESP32, `Adc::new` sets the result inversion bit only for
ADC2, so ADC1 readings (GPIO32–39) came out mirrored (`4095 - value`; an unconnected pin read 4095 instead of 0).
`Board::new` sets `SENS.sar_read_ctrl.sar1_data_inv` like ESP-IDF does.
| [`scripts/watchdog.py`](micropython-esp32/scripts/watchdog.py) | The watchdog resetting a hung script, and `reset_cause()` after the reboot |
| [`scripts/errors.py`](micropython-esp32/scripts/errors.py) | A forbidden pin reported as `ValueError` with its line number |

### Yard lights app

[`scripts/yard_lights.py`](micropython-esp32/scripts/yard_lights.py) switches an LED strip on at sunset and off at
sunrise, every day, unattended for months: the board computes sunrise and sunset for its location, gets the time from
NTP over Wi-Fi and follows the EU summer time rules. It uses `machine.Pin`, `machine.WDT`, `network.WLAN`, `ntptime`,
`time` and `math`, and, on the TTGO, `display` and `buttons` (without them it runs headless and reports to the serial
port).

```sh
# edit the configuration at the top of the script (Wi-Fi, location, pin), then:
make flash FEATURES=tft,wifi
python3 tools/mpy.py save scripts/yard_lights.py     # runs at every power-up
```

| Setting | Default | Meaning |
|---------|---------|---------|
| `WIFI_SSID`, `WIFI_PASSWORD` | | The network (a 2.4 GHz IoT network is fine: it's only used for NTP) |
| `LATITUDE`, `LONGITUDE` | 52.4064, 16.9252 (Poznań) | Degrees, north and east positive |
| `UTC_OFFSET`, `EU_SUMMER_TIME` | 1, `True` | Poland: CET (UTC+1), CEST (UTC+2) from the last Sunday of March to the last Sunday of October, 01:00 UTC |
| `LED_PIN`, `LED_ACTIVE_HIGH` | 26, `True` | The pin to the MOSFET or relay; `False` for relay modules that switch on with a low input |
| `SUN_ANGLE` | -0.833 | The sun's height at the switch: -0.833 is sunrise/sunset, -6 civil dusk/dawn (about 35 min later/earlier) |
| `ON_DELAY_MIN`, `OFF_ADVANCE_MIN` | 0, 0 | Minutes after sunset / before sunrise |
| `NIGHT_BREAK` | `None` | `((0, 30), (5, 0))`: off from 00:30 to 05:00 local time, to save power |
| `LIGHTS_WITHOUT_CLOCK` | `False` | The lights while the clock isn't set yet (no Wi-Fi after a power cut) |
| `NTP_HOST`, `SYNC_HOURS` | `pl.pool.ntp.org`, 6 | Where and how often to set the clock |
| `SCREEN_SECONDS`, `WATCHDOG_MS` | 60, 60000 | The screen goes dark after this long; the board resets if the script stops for this long |

The screen shows the state (ON in orange, OFF), why (after sunset, daylight, night break, manual), the next switch,
today's sunrise and sunset, the clock's state and the Wi-Fi signal. Buttons: LEFT wakes the screen, holding it
switches the lights by hand until the next scheduled switch; RIGHT shows the details page (location, time zone, last
sync, IP address, time running), holding it sets the clock from NTP now.

**Wiring.** A 12 V LED strip needs a switch between the strip and its supply: a logic-level N-channel MOSFET (gate to
GPIO26 through 100 Ω, 100 kΩ from gate to ground, drain to the strip's minus, source to ground), or a 3.3 V relay
module. Connect the grounds of the supply and the board. Power the board from the strip's supply through a 5 V
step-down converter. Don't drive the strip from the GPIO itself: a pin gives 20-40 mA.

**Running for months.** The clock is set again every 6 hours (the ESP32's clock drifts a few seconds a day). When
Wi-Fi or NTP fails, it retries after 1, 2, 4 ... up to 30 minutes, and Wi-Fi reconnects when the router comes back.
The watchdog resets the board if the script hangs or stops on an error; after a reset the clock is gone until
the first NTP answer (seconds when Wi-Fi works). Sunrise and sunset use NOAA's solar equations (Meeus): within 24 s of
[astral](https://pypi.org/project/astral/) for Poznań over 2026-2028 (36 s for civil dusk), and the time zone matched
CPython's `Europe/Warsaw` at every hour of those 3 years. Polar day and night are handled (off all day, on all day).

Measured on the TTGO (`FEATURES=tft,wifi`, 80 MHz): the first sync with connecting to Wi-Fi 1.1-4.7 s; a schedule check
(every second) 9 ms; a redraw 80-110 ms; sunrise/sunset for a day 80 ms, done twice a day. Stack at most 81 KB of the
102 KB, heap 19 KB and 137 objects free. Switching checked by setting the clock to a minute before and after sunset,
sunrise and the night break; after Ctrl-C at the `>>>` prompt, the watchdog reset the board after 62 s and `main.py`
started again.

Nice to have: a DS3231 RTC module on I²C (keeps the time without Wi-Fi after a power cut), a light sensor (an LDR on
GPIO36) to switch on early on dark days, PWM dimming (`machine.PWM` on the MOSFET's gate, e.g. 30 % after midnight),
and a motion sensor (PIR, `Pin.irq()`) for full brightness when someone walks by.

### Hardware API

The names follow MicroPython's `time` and `machine` modules, as flat functions. The [`machine` module](#the-machine-module)
wraps them in MicroPython's classes (`Pin`, `ADC`, `PWM`, `I2C`, `UART`, `SPI`, `WDT`).
Pins are plain GPIO numbers.

| Area | Functions | Notes |
|------|-----------|-------|
| Time | `sleep_ms(ms)`, `sleep_us(us)`, `ticks_ms()`, `ticks_us()`, `ticks_diff(new, old)` | `delay_ms(ms)` is an alias of `sleep_ms`. Ticks count from boot as 64-bit values and don't wrap |
| GPIO | `pin_mode(pin, mode)`, `pin_write(pin, value)`, `pin_read(pin)`, `pin_toggle(pin)` | Modes: `OUT`, `IN`, `IN_PULLUP`, `IN_PULLDOWN`, `OUT_OD` (open drain), `OUT_OD_PULLUP`. `pin_write` on an unconfigured pin makes it an output; `pin_read` on one makes it an input. Outputs read back their own level |
| ADC | `adc_read(pin)`, `adc_mv(pin, [samples])`, `adc_vref()` | `adc_read`: raw 12-bit value (0–4095, 11 dB attenuation) on GPIO34, 36 and 39. `adc_mv`: millivolts, averaged over `samples` readings (default 16), calibrated with the chip's eFuse reference voltage (`adc_vref()`, e.g. 1135 mV). See [ADC accuracy](#adc-accuracy) |
| PWM | `pwm(pin, freq, duty)`, `pwm_u16(pin, freq, duty)`, `pwm_stop(pin)` | `duty` in percent (0–100), or for `pwm_u16` as 0–65535 (MicroPython's `duty_u16`). Up to 4 pins at once, each with its own frequency (LEDC timers 0–3). The duty resolution adapts to the frequency (14 bits at 50 Hz for servos). `pwm_stop` leaves the pin as a low output |
| I²C | `i2c_init(sda, scl, [freq])`, `i2c_scan()`, `i2c_write(addr, data)`, `i2c_read(addr, n)`, `i2c_write_read(addr, data, n)`, `i2c_probe(addr)`, `i2c_read_reg(addr, reg)`, `i2c_read_reg16(addr, reg)`, `i2c_write_reg(addr, reg, value)` | 100 kHz on SDA = GPIO21, SCL = GPIO22 at boot; `i2c_init` moves the bus to other allowed pins or changes the speed (1 kHz – 1 MHz). `i2c_scan()` returns the addresses as `bytes` (`for a in i2c_scan(): print(hex(a))`). `data` is `bytes` (or a `str`); reads return `bytes`, up to 512 at a time. `i2c_write_read` writes (e.g. a register number) and reads back without a stop in between. 16-bit reads are big-endian. A device that doesn't answer raises `OSError` |
| UART | `uart_init(tx, rx, [baud])`, `uart_write(data)`, `uart_read([n], [timeout_ms])`, `uart_any()` | UART1 on any two allowed pins, default 115 200 baud (300 to 5 000 000). `uart_write` returns the bytes sent; `uart_read` waits until `n` bytes (default 512) or `timeout_ms` (default 100) and returns what came, possibly `b''`; `uart_any()` counts the bytes waiting. Ctrl-C stops a wait. `uart_init` again changes pins or speed |
| SPI | `spi_init(sck, mosi, miso, [khz], [mode])`, `spi_write(data)`, `spi_transfer(data)`, `spi_read(n, [fill])` | SPI3 master, default 1000 kHz (10 to 40 000) and mode 0 (0–3). Chip select is yours: a GPIO with `pin_write`. `spi_transfer` returns the bytes read while sending; `spi_read` sends `fill` (default 0) |
| Watchdog | `wdt_enable(timeout_ms)`, `wdt_feed()`, `wdt_disable()` | The RTC watchdog resets the whole board when it isn't fed in time. `sleep_ms` feeds it automatically every 100 ms; the `>>>` prompt doesn't, so after a script ends (an error, Ctrl-C) the board resets and runs `main.py` again |
| Clock | `rtc_set(seconds, [microseconds])`, `wifi_ntp(host, [timeout_ms])` (`wifi` only) | The wall clock behind `time.time()`, in seconds since 2000-01-01 UTC (MicroPython's ESP32 epoch). It starts at 2000-01-01 after a reset (no battery) and survives soft resets. `wifi_ntp` asks an NTP server (a name, looked up with the router's DNS server, or an IP address) and returns microseconds since 2000; `OSError` when it doesn't answer. Use them through `ntptime` and `machine.RTC` |
| System | `reset()`, `reset_cause()`, `stack_peak()`, `unique_id()`, `cpu_freq()`, `cpu_info()`, `cpu_bench(name, [pin])` | `reset_cause()` returns `PWRON_RESET`, `HARD_RESET`, `WDT_RESET`, `DEEPSLEEP_RESET` or `SOFT_RESET` (MicroPython's values). The firmware also prints the reset reason at boot. `stack_peak()`: the most bytes of the main stack used since its previous call (or boot), see [Stack budget](#stack-budget-measured-on-the-esp32). `unique_id()`: the chip's MAC address (6 bytes). `cpu_freq()`: the CPU clock in Hz (`CPU_MHZ`, 80 MHz by default) |
| Battery (`tft` only) | `battery_mv([samples])` | Battery voltage in mV: switches on the TTGO's divider (`ADC_EN`, GPIO14), reads GPIO34, doubles it, switches the divider off |
| TFT (`tft` only) | `cls()`, `color(r, g, b)`, `text(x, y, s)`, `rect(x, y, w, h)`, `backlight(on)` | `color` sets the text and fill color (0–255 per channel). `text` and `rect` draw at pixel positions. `WIDTH` and `HEIGHT` are 240 and 135 |

#### The `machine` module

MicroPython's hardware classes, so code from MicroPython tutorials runs unchanged:

```python
from machine import Pin, ADC, PWM, I2C, UART
import time

led = Pin(2, Pin.OUT)
button = Pin(0, Pin.IN, Pin.PULL_UP)
while button.value():               # until the BOOT button is pressed
    led.value(not led.value())
    time.sleep_ms(250)

adc = ADC(Pin(34), atten=ADC.ATTN_11DB)
print(adc.read_u16(), adc.read_uv())             # 0-65535, calibrated microvolts
servo = PWM(Pin(13), freq=50, duty_ns=1500000)   # 1.5 ms pulses
i2c = I2C(0, scl=Pin(22), sda=Pin(21), freq=400000)
print(i2c.scan(), i2c.readfrom_mem(0x68, 0x75, 1))
gps = UART(2, baudrate=9600, tx=17, rx=26, timeout=1000)
print(gps.readline())
```

It's Python ([`modules/machine.py`](micropython-esp32/modules/machine.py)) on top of the functions above, built into
the firmware as a *frozen module* (`Interpreter::with_frozen_modules`): imported like a file, without using the module
memory. A `machine.py` on the board's file system comes first, as in MicroPython. Cargo feature `machine` (on by default).

| Class | Supported |
|-------|-----------|
| `Pin(id, mode, pull, value=)` | `Pin.IN`, `Pin.OUT`, `Pin.OPEN_DRAIN`, `Pin.PULL_UP`, `Pin.PULL_DOWN`; `value([v])`, `on()`, `off()`, `toggle()`, `init(...)`, `pin()` / `pin(v)` |
| `ADC(pin, atten=)` | `read_u16()`, `read()` (12-bit), `read_uv()` (calibrated), `atten()`, `width()`; only `ATTN_11DB` and `WIDTH_12BIT` (others raise `ValueError`) |
| `PWM(pin, freq=, duty=, duty_u16=, duty_ns=)` | `freq([f])`, `duty([0-1023])`, `duty_u16([0-65535])`, `duty_ns([ns])`, `init(...)`, `deinit()`; up to 4 pins |
| `I2C(id, scl=, sda=, freq=)`, `SoftI2C(scl, sda, freq=)` | `scan()`, `readfrom()`, `writeto()`, `readfrom_mem()`, `writeto_mem()` (also `addrsize=16`); one bus (pins default to 22 and 21) |
| `UART(id, baudrate, tx=, rx=, timeout=)` | `write()`, `read([n])` (`None` when nothing came), `readline()`, `any()`, `init(...)`; id 1 or 2 (UART0 is the REPL), one at a time, 8N1 only; `UART(2)` defaults to tx 17, rx 16 |
| `SPI(id, baudrate, polarity=, phase=, sck=, mosi=, miso=)`, `SoftSPI` | `read(n, [write])`, `write()`, `init(...)`; one bus, 8 bits MSB first; MicroPython's default pins for ids 1 and 2 |
| `Pin.irq(handler=, trigger=)` | `handler(pin)` after an edge: `Pin.IRQ_RISING`, `Pin.IRQ_FALLING`, or both (`IRQ_RISING \| IRQ_FALLING`, the default); `irq(handler=None)` turns it off. Level triggers (`IRQ_LOW_LEVEL`, `IRQ_HIGH_LEVEL`) raise `ValueError` |
| `Timer(id, mode=, period=, freq=, callback=)` | `Timer.PERIODIC`, `Timer.ONE_SHOT`; `period` in ms (`tick_hz=` changes the unit) or `freq` in Hz; `init(...)`, `deinit()`; `callback(timer)`. Ids 0-3 (and -1), software timers |
| `WDT(timeout=)` | `feed()` |
| functions | `reset()`, `reset_cause()` and its constants, `unique_id()`, `freq()` (fixed when building: `CPU_MHZ`; another value raises `ValueError`), `lightsleep(ms)` (waits) |

```python
from machine import Pin, Timer
import time

presses = 0
def pressed(pin):
    global presses
    presses += 1
Pin(0, Pin.IN, Pin.PULL_UP).irq(handler=pressed, trigger=Pin.IRQ_FALLING)   # the BOOT button

led = Pin(2, Pin.OUT)
blink = Timer(0, mode=Timer.PERIODIC, period=250, callback=lambda t: led.toggle())
while presses < 3:
    time.sleep_ms(100)           # callbacks run while the script sleeps
blink.deinit()
```

Pin interrupts and timer callbacks are *soft interrupts*, like MicroPython's default (`hard=False`; `hard=True` is
accepted and works the same): the interrupt (or the timer, in software) only records the event, and the callback runs
at the next statement, while the script sleeps (a sleep stops for it and goes on afterwards), or at the `>>>` prompt
when the script has ended (so a timer keeps going, as in MicroPython; not in the raw REPL Thonny uses). Callbacks
don't interrupt each other; an error in one is printed (`Unhandled exception in IRQ callback handler`) and the
script goes on; a soft reset (Ctrl-D) stops them all. Measured on the TTGO: a 50 ms timer fires 20 times in a
`sleep_ms(1000)`, a 100 Hz timer 50 times in a 500 ms busy loop, 5 pulses on a pin give 10 callbacks with both
edges; with a timer set, scripts run about 6 % slower (an event check per statement), without one at the same speed.

`machine.RTC()` keeps the time as on MicroPython's ESP32 port: `rtc.datetime()` is `(year, month, day, weekday,
hours, minutes, seconds, microseconds)`, `rtc.datetime((2026, 11, 26, 0, 7, 32, 0, 0))` sets it (the weekday is
ignored). The ESP32 has no battery for it: after a reset or power-up it starts at 2000-01-01 again, so set it from
NTP (`ntptime.settime()`) or keep another clock (a DS3231 module on I²C). It drifts a few seconds a day.

`machine.lscpu()` describes the board like Linux's `lscpu`: a dict with `Architecture`, `CPU(s)`, `Model name`,
`Revision`, `CPU MHz`, `Flags`, `Flash size`, `PSRAM size`, `Stack size`, the Python and Wi-Fi heaps, `MAC address`, `Board`, `Firmware`, `Commit`, `Build`,
`Reset cause` and more (sizes in bytes); `machine.lscpu(True)` prints it as a table instead (sizes in KiB/MiB). It's
not in MicroPython. The firmware's `cpu_info()` gives the dict itself (built in Rust; `lscpu` adds the Python heap). On the three boards it was tried on
(each matched what `espflash board-info` reads from the chip):

| | TTGO T-Display | Seeed XIAO ESP32-S3 | ESP32-C3 Super Mini |
|-|----------------|---------------------|---------------------|
| Architecture, CPU core | xtensa, Xtensa LX6 | xtensa, Xtensa LX7 | riscv32, RISC-V RV32IMC |
| Model name, Revision | ESP32, v1.1 | ESP32-S3, v0.2 | ESP32-C3, v0.4 |
| CPU(s), CPU max MHz | 2, 240 | 2, 240 | 1, 160 |
| Flags | fpu wifi bt ble | fpu wifi ble usb-jtag | wifi ble usb-jtag |
| Flash size, SRAM size | 16 MiB, 520 KiB | 8 MiB, 512 KiB | 4 MiB, 400 KiB |
| Stack size (Wi-Fi builds) | 102 KiB | 140 KiB | 115 KiB |

```text
>>> import machine
>>> machine.lscpu(True)
Architecture:      riscv32
Byte Order:        Little Endian
CPU(s):            1
Vendor ID:         Espressif
Model name:        ESP32-C3
CPU core:          RISC-V RV32IMC
Revision:          v0.4
CPU MHz:           80
CPU max MHz:       160
Flags:             wifi ble usb-jtag
Flash size:        4 MiB
SRAM size:         400 KiB
Stack size:        115 KiB
Python heap size:  32 KiB
Python heap free:  15 KiB
Wi-Fi heap size:   72 KiB
Wi-Fi heap free:   72 KiB
MAC address:       34:b7:da:f8:c7:dc
Board:             ESP32-C3 Super Mini (or another ESP32-C3)
Firmware:          0.1.0
Build:             full wifi
Uptime (ms):       4709
Reset cause:       soft reset
```

`machine.bench()` measures the board: a dict `{name with unit: value}`; `machine.bench(True)` prints it as a table.
Native tests (the firmware's `cpu_bench(name, [pin])`, in [`src/bench.rs`](micropython-esp32/src/bench.rs)) run
without the interpreter: integer and float arithmetic (f32 and f64 both, whatever the build's float), RAM and flash,
and pin toggles through esp-hal and straight through the GPIO registers. Python tests show what a script gets. The
GPIO tests use the board's LED pin, or `bench(gpio=2)`; `gpio=None` skips them. Each native test is a chain of
dependent operations (the compiler can't fold it) repeated until it runs 20 ms: it measures latency, the time code
waits for each result. About 3 s per run. Measured at 80 MHz (`CPU_MHZ` default) with the Wi-Fi builds:

| Test (80 MHz) | ESP32 (TTGO) | ESP32-S3 (XIAO) | ESP32-C3 (Super Mini) |
|---|---:|---:|---:|
| native i32 add (Mop/s) | 49.22 | 53.33 | 53.33 |
| native i32 mul (Mop/s) | 19.39 | 21.33 | 30.48 |
| native i32 div (Mop/s) | 8.09 | 8.19 | 3.88 |
| native i64 add (Mop/s) | 10.32 | 10.49 | 17.78 |
| native i64 mul (Mop/s) | 8.89 | 9.14 | 6.81 |
| native i64 div (Mop/s) | 0.6 | 0.61 | 0.37 |
| native count leading zeros (Mop/s) | 21.33 | 22.86 | 2.79 |
| native f32 add (Mop/s) | 18.28 | 19.39 | 0.85 |
| native f32 mul (Mop/s) | 18.28 | 19.39 | 0.62 |
| native f32 div (Mop/s) | 0.46 | 1.55 | 0.41 |
| native f32 sqrt (Mop/s) | 0.53 | 0.59 | 0.26 |
| native f32 sin (Mop/s) | 0.07 | 0.07 | 0.03 |
| native f64 add (Mop/s) | 1.62 | 1.57 | 0.66 |
| native f64 mul (Mop/s) | 0.81 | 0.83 | 0.35 |
| native f64 div (Mop/s) | 0.16 | 0.18 | 0.21 |
| native f64 sqrt (Mop/s) | 0.16 | 0.16 | 0.13 |
| native f64 sin (Mop/s) | 0.03 | 0.03 | 0.02 |
| native RAM write (MiB/s) | 235.3 | 234.8 | 149.9 |
| native RAM read (MiB/s) | 63.9 | 83.6 | 121.1 |
| native RAM copy (MiB/s) | 118.7 | 118.4 | 108.4 |
| native flash read, cached (MiB/s) | 51.9 | 83.6 | 7.6 |
| native GPIO toggle, esp-hal (M/s) | 3.62 | 3.18 | 5.38 |
| native GPIO toggle, register (M/s) | 9.7 | 10.16 | 11.64 |
| native GPIO read, register (M/s) | 7.44 | 5.12 | 12.31 |
| Python for loop (k/s), before the [statement cache](#statement-cache) | 11.0 | 12.7 | 12.4 |
| Python int math (x*3+i)%k (k/s) | 3.9 | 4.5 | 4.6 |
| Python float math y*a+b (k/s) | 4.0 | 4.6 | 4.7 |
| Python function call (k/s) | 3.4 | 3.8 | 3.9 |
| Python list append (k/s) | 4.0 | 4.5 | 4.5 |
| Python dict set (k/s) | 2.7 | 3.3 | 3.1 |
| Python str format (k/s) | 1.1 | 2.0 | 1.5 |
| Python pin_write (k/s) | 1.9 | 2.5 | 2.4 |
| Python Pin.value (k/s) | 0.7 | 0.9 | 0.9 |

What it shows:
* **Single precision FPU** (ESP32, S3): f32 add and mul are 20-30x faster than on the C3, which has no FPU (and
  doubles run in software everywhere: 10-20x slower than f32 on the Xtensa chips). `make flash FLOAT=32` gives
  scripts these f32 speeds (see [Single precision](#single-precision-float32)). Division: the S3's FPU has division
  steps the ESP32's lacks (3.4x faster). `sin` goes through libm, which reduces `sinf`'s argument in double precision.
* **Bit counting**: Xtensa has `NSAU` (count leading zeros in one instruction); RV32IMC has no bit-manipulation
  extension, so the C3 does it in software (8x slower).
* **Integers**: the C3's multiplier is faster (RISC-V M extension), its divider slower; 64-bit division is a
  software routine on all three (40-50x slower than 64-bit add).
* **Memory**: internal RAM moves 100-235 MiB/s at 80 MHz. Flash goes through a cache (32 KB or more on the ESP32 and
  S3, 16 KB for code and data together on the C3): the 16 KB that's read stays in the Xtensa chips' caches, not in
  the C3's, which then reads the flash itself (7.6 MiB/s).
* **GPIO**: a register write toggles a pin at about 10 MHz (5 MHz square wave), esp-hal's `Flex` at 3-5 MHz, a
  script's `pin_write` at 2.5 kHz and `Pin.value()` at about 1 kHz: for anything faster than a few kHz, use PWM or a
  bus (SPI, UART, I2C).
* **The interpreter**: 11-13 thousand empty loop passes a second, about 1000x below native code; the chip matters
  less than the interpreter. `CPU_MHZ=240` (160 on the C3) runs everything 2-3x faster.

Not there yet: level triggers, `Signal`, and the `*_into` methods (`readinto`, `write_readinto`,
`readfrom_mem_into`), which need `bytearray`. The module takes 53 of the firmware's 96 function slots (112 on the C3, 128 on the S3), and each
method call runs Python code: `led.value(1)` takes 1.2 ms
on the ESP32 at 80 MHz (0.43 ms at 240 MHz), `pin_write(2, 1)` 0.27 ms (0.12 ms). Fine for blinking, sensors and
buses, too slow for bit-banging.

#### The `network` module (Wi-Fi)

With `FEATURES=wifi` (`make flash FEATURES=tft,wifi` on the TTGO), MicroPython's `network.WLAN` connects the board to a
Wi-Fi network, with the same calls as on MicroPython's ESP32 port:

```python
import network, time

wlan = network.WLAN(network.STA_IF)
wlan.active(True)
print(wlan.scan())                       # [(b'my-network', b'\x..', 5, -68, 3, False), ...]
wlan.connect('your-network', 'your-password')
while not wlan.isconnected():
    if wlan.status() in (network.STAT_WRONG_PASSWORD, network.STAT_NO_AP_FOUND):
        raise OSError(wlan.status())
    time.sleep_ms(100)
print(wlan.ifconfig())                   # ('192.168.0.111', '255.255.255.0', '192.168.0.1', '8.8.8.8')
```

| Call | Does |
|------|------|
| `network.WLAN(network.STA_IF)` | The station (client) interface; `network.WLAN(network.AP_IF)` is the access point (see below) |
| `active([on])` | Starts or stops the radio; returns whether it's on. The first `active(True)` starts the Wi-Fi stack (93 ms) |
| `scan()` | `(ssid, bssid, channel, RSSI, security, hidden)` tuples, `ssid` and `bssid` as bytes, `security` an `AUTH_*` value; up to 120 ms per channel like ESP-IDF (0.5-1 s). The radio doesn't scan while the station is connecting (after `connect()`, or after `WLAN(AP_IF).active(...)` restarts the radio and the station reconnects): `scan()` waits for that, up to 5 s, then raises `OSError` (a wrong password keeps the station trying) |
| `connect(ssid, key)` | Starts connecting and returns, like MicroPython; DHCP follows. WPA2 or better (WPA3 too), or an open network with no key |
| `isconnected()`, `status()` | `True` once there's an IP address; `STAT_IDLE`, `STAT_CONNECTING`, `STAT_GOT_IP`, `STAT_WRONG_PASSWORD`, `STAT_NO_AP_FOUND`, `STAT_BEACON_TIMEOUT`, `STAT_ASSOC_FAIL`, `STAT_HANDSHAKE_TIMEOUT` (MicroPython's ESP32 values) |
| `status('rssi')` | Signal strength in dBm |
| `ifconfig()` | `(ip, netmask, gateway, dns)` from DHCP, `'0.0.0.0'` when not connected |
| `config('mac')`, `config('ssid')`, `config('hostname')`, `config(hostname=...)` | MAC address (bytes), the network, the host name (`network.hostname()` too) |
| `disconnect()` | Leaves the network |
| constants | `STA_IF AP_IF`, `STAT_*`, `AUTH_OPEN AUTH_WEP AUTH_WPA_PSK AUTH_WPA2_PSK AUTH_WPA_WPA2_PSK AUTH_WPA2_ENTERPRISE AUTH_WPA3_PSK AUTH_WPA2_WPA3_PSK`, and `WLAN.IF_STA IF_AP SEC_*` |

Measured on the TTGO T-Display (80 MHz) with a WPA2 home network: the radio starts in 93 ms, a scan takes 0.5-1 s
(5-19 networks around), `connect()` to `isconnected()` takes 2.7-5.3 s with DHCP, a wrong password is reported
(`STAT_WRONG_PASSWORD`) after about 8 s and an unknown network (`STAT_NO_AP_FOUND`) after about 1.7 s. The board
answers pings from the network while a script runs or sleeps (39-139 ms round trips: it's polled every 100 ms); the
connection stays up across soft resets and Thonny sessions.

**Access point.** The board can also run its own Wi-Fi network, or both at once (`ApSta`: the access point then uses
the station's channel):

```python
ap = network.WLAN(network.AP_IF)
ap.config(ssid='my-board', key='change-me-123')    # WPA2 with a key, an open network without
ap.active(True)                                     # the board is 192.168.4.1
print(ap.ifconfig(), ap.status('stations'))         # (..), [(b'\x02\x00\x00\x00\x00\x01',)]: the devices joined
```

| Call (`AP_IF`) | Does |
|------|------|
| `active([on])` | Starts or stops the access point; the default name is `ESP_` and the end of its MAC address, open, channel 1 |
| `config(ssid=, key=, authmode=, channel=, hidden=, max_clients=)` | Also `essid=`, `password=`, `security=`; a `key` without `authmode` means `AUTH_WPA2_PSK` (8-63 characters), `AUTH_OPEN` without a key; channels 1-13; up to 8 devices. `config('ssid')`, `'channel'`, `'authmode'`, `'hidden'`, `'max_clients'`, `'mac'` read them back |
| `ifconfig()` | `('192.168.4.1', '255.255.255.0', '192.168.4.1', '0.0.0.0')` |
| `status('stations')`, `isconnected()` | `(mac,)` for each device joined; whether any has |

Devices that join get an address from a small DHCP server in the firmware (192.168.4.2 and up, 2-hour leases; the
board is their router, with no DNS). Tested on the TTGO with a Linux laptop: it found `my-board` (WPA2) and
`mpy-rs-open` (open) at full signal, got 192.168.4.2 from the board in a few seconds, and pinged it: 50/50 (2.8-27 ms
while the board waited in the raw REPL), 20/20 with 1400-byte packets; with the station connected to the home
network at the same time, the board answered on both (192.168.0.111 and 192.168.4.1) and the laptop's own wired
connection stayed its default route. `status('stations')` showed the laptop's MAC address while it was joined.

How it works ([`src/wifi.rs`](micropython-esp32/src/wifi.rs), [`modules/network.py`](micropython-esp32/modules/network.py)):
the driver is [`esp-radio`](https://crates.io/crates/esp-radio) (Espressif's Wi-Fi library) on the
[`esp-rtos`](https://crates.io/crates/esp-rtos) scheduler, both started by the first `active(True)`, and DHCP and IP
come from [`smoltcp`](https://crates.io/crates/smoltcp) (two interfaces, the station's with a DHCP client and the
access point's with the firmware's DHCP server). There's no background network task: the interfaces are polled when a
script calls `status()`, `isconnected()` or `ifconfig()`, while it sleeps, and while the REPL waits for input (also
the raw REPL, so the access point keeps serving under Thonny), but not while a script computes without sleeping.
`network` is a frozen Python module over the firmware's `wifi_*` functions (`wifi_connect(ssid, key)`,
`wifi_ap_config(...)`, ...). The driver gets fewer buffers than its defaults (6 + 12 receive, 12 transmit), sized
for a microcontroller with a few devices.

What Wi-Fi costs (the reason it's a feature, not in every build):

| | Without `wifi` | With `wifi` |
|-|---------------:|------------:|
| App image (TTGO) | 424 KB | 882 KB |
| IRAM | 48 KB | 101 KB (the driver's interrupt code) |
| Static RAM | 11 KB | 89 KB (a 72 KB heap for the driver: 37 KB used with the station, 49 KB with the station and the access point, 55 KB at most with a device joined, pings and scans) |
| Main stack | 181 KB | 102 KB (recursion ends with `RecursionError` at about 95 KB instead of 154 KB) |
| Speed with Wi-Fi off / connected | | the same / 2 % slower (the scheduler's ticks) |

The time over Wi-Fi, as on MicroPython:

```python
import ntptime, time
ntptime.host = 'pl.pool.ntp.org'   # default: pool.ntp.org; ntptime.timeout = 1 (seconds)
ntptime.settime()                  # sets the clock (UTC); OSError when no server answers
print(time.localtime())            # (2026, 11, 25, 23, 30, 6, 2, 329): UTC, the board has no time zone
```

The firmware looks names up with smoltcp's DNS client (the DNS server DHCP gave, or 8.8.8.8), then sends one SNTP
request. Measured on the TTGO: `settime()` takes 23-77 ms, an unknown name fails in 40 ms; about 1 query in 10 gets
no answer (UDP), and NTP servers ignore a repeat within a few seconds, so retry after a pause.

Not there yet: static addresses (`ifconfig((...))`), and sockets (`socket`, `urequests`), which are next. `network`
takes 12 function slots, `time` and `ntptime` 2 each, so a script using `machine`, `network` and `ntptime` has 27 of
the 96 left.

#### Modules

| Module | Builds | Contents |
|--------|--------|----------|
| `machine` | both | MicroPython's hardware classes, see [The `machine` module](#the-machine-module) |
| `network` | `wifi` | Wi-Fi: `network.WLAN`, see [The `network` module](#the-network-module-wi-fi) |
| `time` | both | `sleep(s)` (also a float: `sleep(0.5)`), `sleep_ms(ms)`, `sleep_us(us)`, `ticks_ms()`, `ticks_us()`, `ticks_diff(new, old)`, `ticks_add(t, delta)`, `time()`, `time_ns()`, with MicroPython's names, so `import time; time.sleep_ms(100)` works as on MicroPython. At the `full` level also `gmtime([secs])`, `localtime([secs])` (the same: no time zone) and `mktime(tuple)`, native (`micropython_rs::calendar`, about 0.3 ms a call on an ESP32-S3 at 80 MHz instead of 29 ms in Python). `time()` counts seconds since 2000-01-01 UTC, MicroPython's ESP32 epoch |
| `ntptime` | `wifi` | `settime()`, `time()`, `host`, `timeout`: the clock from an NTP server |
| `gc` | both | `collect()`, `mem_free()`, `mem_alloc()`, `objects_free()` (free object-table entries, not in MicroPython), `enable()`/`disable()`/`isenabled()` (collection is always on) |
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
| `backlight(on)` | Backlight off (0) or on (1); the picture stays. It's on again when a script ends |
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
| `micropython-esp32/src/storage.rs` | Finds the `vfs` partition, implements the file system's `Flash` trait with `esp-storage`, and moves an old `nvs` `main.py` into the file system |
| `micropython-esp32/partitions.csv` | Partition table: `nvs`, `phy_init`, 3 MB `factory` app, 960 KB `vfs` for files |
| `micropython-esp32/tools/mpy.py` | Host tool: run, save, show and erase scripts over USB serial |
| `micropython-esp32/scripts/` | The demo and example scripts |
| `micropython-esp32/build.rs` | Picks the script (`MPY_SCRIPT`, or the board's demo) and hands its path to `include_str!` |
| `micropython-esp32/Cargo.toml` | Declares the `tft` feature. `mipidsi`, `embedded-graphics` and `embedded-hal-bus` are optional dependencies, so boards without a display don't pull them in |
| `micropython-esp32/Makefile` | Docker build and host flash/monitor (with `partitions.csv`), with the `FEATURES`, `SCRIPT`, `ERASE_FILES` and `PORT` variables |

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
I (58) boot:  2 factory          factory app      00 00 00010000 00300000
I (64) boot:  3 vfs              Unknown data     01 06 00310000 000f0000
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

micropython-rs 0.1.0 (e3f938c) REPL on ESP32
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

micropython-rs 0.1.0 (e3f938c) REPL on ESP32
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
- [x] Calibrated ADC readings (`adc_mv`, eFuse Vref), `battery_mv()`, and a voltmeter app with a live graph
- [x] Strings, bytes, containers, exceptions, classes, closures, files and `os`: Thonny 3.2.7–5.0 and mpremote work
- [x] Multi-byte I²C, UART and SPI with `bytes`; importing your own `.py` modules
- [ ] Compact Bytecode Compiler & Stack VM


## Contributing

Contributions are welcome! Whether you are fixing a typo, adding a new Python built-in, or improving docs for fellow Rust learners, feel free to open an issue or submit a pull request.

What CI runs, and how to run it yourself:

| Check | Command |
|-------|---------|
| Formatting, lints, tests, coverage ≥ 30 %, `no_std` builds | `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `make all` |
| Language features against CPython | `cargo test` runs `tests/cpython/*.py` and compares with the `.out` files (`make cpython-expected` writes them with CPython) |
| Thonny and mpremote end to end | see the `build-and-test` job in [`.github/workflows/ci.yml`](.github/workflows/ci.yml) |
| Both ESP32 builds, memory report | `cd micropython-esp32 && make build [FEATURES=tft] && make report`: fails when the main stack drops below 128 KB or a function takes more than 32 KB of it ([`tools/fw_report.py`](micropython-esp32/tools/fw_report.py)) |
| Fuzzing (5 minutes in CI) | `cargo +nightly fuzz run eval fuzz/corpus/eval -- -dict=fuzz/fuzz.dict -max_total_time=300`: random scripts through the whole interpreter ([`fuzz/fuzz_targets/eval.rs`](fuzz/fuzz_targets/eval.rs)); seed the corpus with `tests/cpython/*.py` first |


## License

Licensed under either of Apache License, Version 2.0

