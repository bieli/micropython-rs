//! The board-independent part of micropython-rs firmware: the interactive
//! REPL, MicroPython's raw REPL (for `mpremote`, Thonny and `tools/mpy.py`)
//! and the `#@` file commands.
//!
//! A board implements [`Port`]: how bytes reach its console and leave it, the
//! lowest stack address scripts may use, and its name and build. It hands
//! over the memory for the sessions ([`Memory`]), its frozen modules, a boot
//! script and, when it has one, a flash file system (`flatfs` on any
//! [`Flash`](micropython_rs::flatfs::Flash)); then [`run`] never returns.
//! `micropython-esp32` (ESP32, ESP32-S3, ESP32-C3) is one such board.
#![no_std]
// The protocol text spells out its line endings (`\r\n` for the raw REPL's
// framing, `\n` for script output): `write!` with them, not `writeln!`
#![allow(clippy::write_with_newline)]

use core::sync::atomic::AtomicBool;

pub mod repl;

pub use repl::{run, BigHeap, BootScript, Memory};

/// Set by Ctrl-C on the console (the board's receive interrupt sets it);
/// stops the running script (checked by the interpreter between statements
/// and inside `sleep_ms`).
pub static ABORT: AtomicBool = AtomicBool::new(false);

/// What the REPL needs from a board. Its functions are associated (the
/// console and the stack are the board's, one of each).
pub trait Port {
    /// The board's chip, in the banner: `micropython-rs ... REPL on ESP32-S3`.
    const NAME: &'static str;
    /// The firmware version and the git commit it's built from (`#@version`,
    /// the banner).
    const VERSION: &'static str;
    const COMMIT: &'static str;

    /// How the firmware was built, as `key=value` words after `#@version`'s
    /// version and commit (`chip=esp32 level=full features=tft,wifi ...`):
    /// `tools/mpy.py` makes the `make flash` command from them.
    fn build_options(out: &mut dyn core::fmt::Write) -> core::fmt::Result;

    /// A byte from the console, if one arrived.
    fn read_byte() -> Option<u8>;

    /// The next byte from the console, waiting for it.
    fn wait_byte() -> u8 {
        loop {
            if let Some(byte) = Self::read_byte() {
                return byte;
            }
        }
    }

    /// Drops received bytes up to and including `byte` (after a Ctrl-C has
    /// stopped a script, so the prompt doesn't see it again).
    fn discard_through(byte: u8);

    /// Writes text straight to the console (the raw REPL's framing goes
    /// there, never to a display).
    fn console_write(s: &str);

    /// The lowest stack address scripts may use: deeper recursion raises
    /// `RecursionError` instead of overflowing the stack.
    fn stack_floor() -> usize;
}
