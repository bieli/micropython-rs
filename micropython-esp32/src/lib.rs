#![no_std]

use core::sync::atomic::AtomicBool;

pub mod board;
pub mod buttons;
pub mod events;
pub mod repl;
pub mod serial_console;
pub mod serial_input;
pub mod stack;
pub mod storage;
#[cfg(feature = "wifi")]
pub mod wifi;

/// Set by Ctrl-C on the serial port; stops the running script
/// (checked by the interpreter between statements and inside `sleep_ms`).
pub static ABORT: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "tft")]
pub mod display_module;
#[cfg(feature = "tft")]
pub mod tft_console;
