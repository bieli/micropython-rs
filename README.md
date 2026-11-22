# micropython-rs
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

## Real ESP32 RISC-V example

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

3. Generate exemplary project for ESP32 microcontroller

```bash
esp-generate --headless -o esp32 -o esp-backtrace micropython-esp32
cd micropython-esp32
```

4. In case of troubles with diff in Rust version between your OS and Xtensa
```bash
mv ~/.rustup/toolchains/esp/bin/cargo ~/.rustup/toolchains/esp/bin/cargo.bak
ln -s "$(rustup which --toolchain stable cargo)" ~/.rustup/toolchains/esp/bin/cargo
```

5. In case of troubles with Rust versions differences (pull the official Xtensa Rust image + build on docker)
```bash
docker pull espressif/idf-rust:esp32_latest
cd ..

docker run --rm \
  -v "$(pwd)":/workspace \
  -w /workspace/micropython-esp32 \
  espressif/idf-rust:esp32_latest \
  bash -c "source /home/esp/export-esp.sh && cargo build --release"
```

6. Flash and monitor you host terminal
```bash
cd micropython-esp32
espflash flash --monitor target/xtensa-esp32-none-elf/release/micropython-esp32
```

This example logs you can see (et the end you see interpreted MicroPython code on embedded device):
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

=== micropython-rs starting on ESP32 ===
Hello from ESP32
a = 21 b = 21
Blinking onboard LED on GPIO2...
LED sequence complete!
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
- [ ] Variables and basic arithmetic (+, -, *, /)
- [ ] Control flow (if / else, while loops with indentation tracking)
- [ ] Compact Bytecode Compiler & Stack VM


## Contributing

Contributions are welcome! Whether you are fixing a typo, adding a new Python built-in, or improving docs for fellow Rust learners, feel free to open an issue or submit a pull request.


## License

Licensed under either of Apache License, Version 2.0

