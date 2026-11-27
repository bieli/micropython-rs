#![no_std]

use core::sync::atomic::AtomicBool;

pub mod bench;
pub mod board;
pub mod buttons;
pub mod chip;
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

/// Bytes of external RAM (PSRAM) the firmware found (`psram` feature; 0 without).
pub static PSRAM_SIZE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[cfg(feature = "tft")]
pub mod display_module;
#[cfg(feature = "tft")]
pub mod tft_console;
