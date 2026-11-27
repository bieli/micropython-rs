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

/// The firmware version and the git commit it's built from (`e3f938c`,
/// `e3f938c-dirty`; `unknown` outside a checkout).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("MPY_GIT_COMMIT");

/// How the firmware was built, as `make` options: `chip=esp32 level=full
/// features=tft,wifi float=64 mhz=80` (`#@version`, `tools/mpy.py`).
pub fn build_options(out: &mut impl core::fmt::Write) -> core::fmt::Result {
    let chip = if cfg!(feature = "esp32s3") {
        "esp32s3"
    } else if cfg!(feature = "esp32c3") {
        "esp32c3"
    } else {
        "esp32"
    };
    let level = if cfg!(feature = "minimal") {
        "minimal"
    } else if cfg!(feature = "medium") {
        "medium"
    } else {
        "full"
    };
    write!(out, "chip={} level={} features=", chip, level)?;
    let mut first = true;
    for (on, name) in [
        (cfg!(feature = "tft"), "tft"),
        (cfg!(feature = "wifi"), "wifi"),
        (cfg!(feature = "psram"), "psram"),
    ] {
        if on {
            write!(out, "{}{}", if first { "" } else { "," }, name)?;
            first = false;
        }
    }
    let float = if cfg!(feature = "float32") { 32 } else { 64 };
    write!(
        out,
        " float={} mhz={}",
        float,
        crate::board::CPU_CLOCK as u32
    )
}

/// A small text buffer on the stack, for `write!` (addresses, versions).
pub struct StackText<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackText<N> {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> core::fmt::Write for StackText<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        self.buf
            .get_mut(self.len..end)
            .ok_or(core::fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// An empty `StackText` of `N` bytes.
pub fn stack_text<const N: usize>() -> StackText<N> {
    StackText {
        buf: [0; N],
        len: 0,
    }
}

/// Bytes of external RAM (PSRAM) the firmware found (`psram` feature; 0 without).
pub static PSRAM_SIZE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[cfg(feature = "tft")]
pub mod display_module;
#[cfg(feature = "tft")]
pub mod tft_console;
