//! Interactive REPL over the USB serial port, plus MicroPython's raw REPL
//! protocol so host tools such as `mpremote` can run scripts.
//!
//! Boot: run `main.py` from flash (or the embedded script), then show the
//! `>>>` prompt. Keys, as in MicroPython:
//!
//! | Key    | Normal REPL                            | Raw REPL                          |
//! |--------|----------------------------------------|-----------------------------------|
//! | Ctrl-A | enter raw REPL                         | restart raw REPL                  |
//! | Ctrl-B | print the banner                       | back to the normal REPL           |
//! | Ctrl-C | stop the running script / clear line   | stop the script / clear input     |
//! | Ctrl-D | soft reset (on an empty line)          | run the input, or soft reset      |
//! | Ctrl-E | paste mode (Ctrl-D runs, Ctrl-C cancels) | raw-paste request (declined)    |
//!
//! Every line that runs stays in memory until the next soft reset, because
//! variables and functions point into their source text. The memory is a
//! fixed arena of [`ARENA_SIZE`] bytes; when it's full, Ctrl-D frees it.

use core::fmt::Write as _;
use core::sync::atomic::Ordering;

use esp_println::print;
use micropython_rs::{Error, HardwareBridge, Interpreter, Output};

use crate::script_store::ScriptStore;
use crate::serial_input::wait_byte;
use crate::ABORT;

/// Bytes of REPL input (and the boot script) kept per session.
pub const ARENA_SIZE: usize = 32 * 1024;

const CTRL_A: u8 = 0x01;
const CTRL_B: u8 = 0x02;
const CTRL_C: u8 = 0x03;
const CTRL_D: u8 = 0x04;
const CTRL_E: u8 = 0x05;
const BACKSPACE: u8 = 0x08;
const TAB: u8 = 0x09;
const ESC: u8 = 0x1B;
const DELETE: u8 = 0x7F;

/// Raw-REPL commands understood by `tools/mpy.py` (a `#@` first line).
const SAVE_COMMAND: &str = "#@save\n";
const ERASE_COMMAND: &str = "#@erase";
const CAT_COMMAND: &str = "#@cat";

/// What the board runs at boot when no `main.py` is saved.
pub struct BootScript {
    pub source: &'static str,
    pub name: &'static str,
}

/// Runs the boot script, then the REPL, forever. A soft reset starts a new
/// interpreter (all variables and functions are cleared).
pub fn run<O: Output, H: HardwareBridge>(
    out: &mut O,
    hw: &mut H,
    boot: &BootScript,
    store: &mut ScriptStore<'_>,
) -> ! {
    let mut memory = [0u8; ARENA_SIZE];
    let mut mode = Mode::Normal;
    let mut run_boot_script = true;

    loop {
        let mut repl = Repl {
            vm: Interpreter::with_hardware(&mut *out, &mut *hw),
            arena: Arena::new(&mut memory),
            history: None,
        };
        repl.vm.set_abort_flag(&ABORT);

        // Like MicroPython, a soft reset in the raw REPL skips main.py
        if run_boot_script {
            repl.run_boot_script(boot, store);
        }

        loop {
            mode = match mode {
                Mode::Normal => repl.normal_mode(),
                Mode::Raw => repl.raw_mode(store),
                Mode::SoftReset(_) => unreachable!(),
            };
            if let Mode::SoftReset(raw) = mode {
                print!("MPY: soft reboot\r\n");
                run_boot_script = !raw;
                mode = if raw { Mode::Raw } else { Mode::Normal };
                break;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    Raw,
    /// Soft reset requested; `true` if it came from the raw REPL.
    SoftReset(bool),
}

/// Append-only source memory: text is collected as "pending" and then
/// committed, which hands out a `&'a str` that stays valid for the session.
struct Arena<'a> {
    free: &'a mut [u8],
    pending: usize,
}

impl<'a> Arena<'a> {
    fn new(memory: &'a mut [u8]) -> Self {
        Self {
            free: memory,
            pending: 0,
        }
    }

    fn push(&mut self, byte: u8) -> bool {
        match self.free.get_mut(self.pending) {
            Some(slot) => {
                *slot = byte;
                self.pending += 1;
                true
            }
            None => false,
        }
    }

    fn pending(&self) -> &[u8] {
        &self.free[..self.pending]
    }

    fn truncate(&mut self, len: usize) {
        self.pending = self.pending.min(len);
    }

    fn clear(&mut self) {
        self.pending = 0;
    }

    /// Moves the pending text out of the free space for good.
    fn commit(&mut self) -> Result<&'a str, Error> {
        let len = self.pending;
        self.pending = 0;
        let free = core::mem::take(&mut self.free);
        let (text, rest) = free.split_at_mut(len);
        self.free = rest;
        let text: &'a [u8] = text;
        core::str::from_utf8(text).map_err(|_| Error::InvalidSyntax)
    }

    /// The free space, for reading a saved script straight into it.
    fn free_space(&mut self) -> &mut [u8] {
        &mut self.free[self.pending..]
    }
}

struct Repl<'a, O: Output, H: HardwareBridge> {
    vm: Interpreter<'a, O, H>,
    arena: Arena<'a>,
    /// Last single line that ran, for the up-arrow key.
    history: Option<&'a str>,
}

/// `core::fmt::Write` for an `Output`, to format error messages.
struct Fmt<'w, O: Output>(&'w mut O);

impl<O: Output> core::fmt::Write for Fmt<'_, O> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.write_str(s);
        Ok(())
    }
}

impl<'a, O: Output, H: HardwareBridge> Repl<'a, O, H> {
    fn write(&mut self, s: &str) {
        self.vm.output().write_str(s);
    }

    fn run_boot_script(&mut self, boot: &BootScript, store: &mut ScriptStore<'_>) {
        let saved = store.load(self.arena.free_space());
        let source = match saved {
            Some(len) => {
                self.arena.pending = len;
                match self.arena.commit() {
                    Ok(source) => {
                        let _ = write!(
                            Fmt(self.vm.output()),
                            "main.py from flash ({} bytes)\n",
                            len
                        );
                        source
                    }
                    Err(_) => {
                        self.write("main.py in flash is not valid UTF-8, skipped\n");
                        return;
                    }
                }
            }
            None => {
                let _ = write!(Fmt(self.vm.output()), "{} (built-in)\n", boot.name);
                boot.source
            }
        };
        self.execute(source, false);
    }

    /// Runs `source` and prints any error, the way the REPL shows it.
    fn execute(&mut self, source: &'a str, interactive: bool) {
        if let Err(err) = self.eval(source, interactive) {
            self.report_error(err);
        }
    }

    fn eval(&mut self, source: &'a str, interactive: bool) -> Result<(), Error> {
        ABORT.store(false, Ordering::Relaxed);
        self.vm.set_interactive(interactive);
        let result = self.vm.eval(source);
        self.vm.set_interactive(false);
        if result == Err(Error::Interrupted) {
            // The Ctrl-C did its job; don't let the prompt see it again
            crate::serial_input::discard_through(crate::serial_input::CTRL_C);
        }
        result
    }

    fn report_error(&mut self, err: Error) {
        let line = self.vm.error_line();
        let mut out = Fmt(self.vm.output());
        let _ = match (err, line) {
            (Error::Interrupted, _) => write!(out, "KeyboardInterrupt\n"),
            (err, Some(line)) => write!(out, "Error at line {}: {:?}\n", line, err),
            (err, None) => write!(out, "Error: {:?}\n", err),
        };
    }

    fn banner(&mut self) {
        self.write(
            "\nmicropython-rs REPL on ESP32\n\
             help() lists functions. Ctrl-C stops a script, Ctrl-D soft resets, Ctrl-E pastes.\n",
        );
    }

    // ------------------------------------------------------------------
    // Normal REPL
    // ------------------------------------------------------------------

    fn normal_mode(&mut self) -> Mode {
        self.banner();
        self.write(">>> ");
        self.arena.clear();

        let mut line_start = 0; // where the line being typed starts in `pending`
        let mut in_block = false; // collecting an indented block (`... ` prompt)
        let mut last_was_cr = false;
        let mut escape = 0u8; // progress through an `ESC [ x` key sequence

        loop {
            let byte = wait_byte();
            let after_cr = core::mem::replace(&mut last_was_cr, byte == b'\r');

            if escape > 0 {
                escape = match (escape, byte) {
                    (1, b'[') => 2,
                    (2, b'A') => {
                        self.recall_history(line_start);
                        0
                    }
                    _ => 0, // other keys (arrows, ...) are ignored
                };
                continue;
            }

            match byte {
                CTRL_A => return Mode::Raw,
                CTRL_B => {
                    self.banner();
                    self.write(">>> ");
                    self.arena.clear();
                    (line_start, in_block) = (0, false);
                }
                CTRL_C => {
                    self.write("\nKeyboardInterrupt\n>>> ");
                    self.arena.clear();
                    (line_start, in_block) = (0, false);
                }
                CTRL_D if self.arena.pending().is_empty() => {
                    self.write("\n");
                    return Mode::SoftReset(false);
                }
                CTRL_E => {
                    self.paste_mode();
                    self.write(">>> ");
                    (line_start, in_block) = (0, false);
                }
                b'\n' if after_cr => {} // the `\n` of a `\r\n` pair
                b'\r' | b'\n' => {
                    self.write("\n");
                    (line_start, in_block) = self.end_line(line_start, in_block);
                }
                BACKSPACE | DELETE => {
                    if self.arena.pending > line_start {
                        self.arena.truncate(self.arena.pending - 1);
                        self.write("\x08 \x08");
                    }
                }
                ESC => escape = 1,
                TAB => {
                    for _ in 0..4 {
                        self.type_byte(b' ');
                    }
                }
                0x20..=0x7E => self.type_byte(byte),
                _ => {} // other control characters and non-ASCII input
            }
        }
    }

    /// Adds a typed character to the line and echoes it.
    fn type_byte(&mut self, byte: u8) {
        if self.arena.push(byte) {
            let buf = [byte];
            self.write(core::str::from_utf8(&buf).unwrap_or("?"));
        } else {
            self.write("\nREPL memory full: press Ctrl-D for a soft reset\n>>> ");
            self.arena.clear();
        }
    }

    /// Up arrow: replaces the current line with the last one that ran.
    fn recall_history(&mut self, line_start: usize) {
        let Some(previous) = self.history else {
            return;
        };
        while self.arena.pending > line_start {
            self.arena.truncate(self.arena.pending - 1);
            self.write("\x08 \x08");
        }
        for byte in previous.bytes() {
            self.type_byte(byte);
        }
    }

    /// Handles Enter: runs the line, or starts/continues/finishes a block.
    /// Returns the new `(line_start, in_block)`.
    fn end_line(&mut self, line_start: usize, in_block: bool) -> (usize, bool) {
        let line = &self.arena.pending()[line_start..];
        let blank = line.iter().all(|&b| b == b' ');
        let opens_block = ends_with_colon(line);

        if in_block && !blank || !in_block && opens_block {
            // Continue the block on the next line
            if self.arena.push(b'\n') {
                self.write("... ");
                return (self.arena.pending, true);
            }
        } else if !in_block && blank {
            self.arena.clear();
        } else {
            // A finished block (ended by an empty line) or a single line
            let single_line = !in_block;
            if in_block {
                self.arena.truncate(line_start); // drop the blank line
            }
            match self.arena.commit() {
                Ok(source) => {
                    if single_line {
                        self.history = Some(source);
                    }
                    self.execute(source, true);
                }
                Err(err) => self.report_error(err),
            }
        }
        self.write(">>> ");
        (0, false)
    }

    /// Ctrl-E: collects pasted text as-is (indentation included) until
    /// Ctrl-D runs it or Ctrl-C cancels.
    fn paste_mode(&mut self) {
        self.write("\npaste mode; Ctrl-C to cancel, Ctrl-D to finish\n=== ");
        self.arena.clear();
        let mut last_was_cr = false;
        loop {
            let byte = wait_byte();
            let after_cr = core::mem::replace(&mut last_was_cr, byte == b'\r');
            match byte {
                CTRL_C => {
                    self.write("\n");
                    self.arena.clear();
                    return;
                }
                CTRL_D => {
                    self.write("\n");
                    match self.arena.commit() {
                        Ok(source) => self.execute(source, false),
                        Err(err) => self.report_error(err),
                    }
                    return;
                }
                b'\n' if after_cr => {}
                b'\r' | b'\n' => {
                    self.arena.push(b'\n');
                    self.write("\n=== ");
                }
                _ => {
                    if self.arena.push(byte) && (0x20..=0x7E).contains(&byte) {
                        let buf = [byte];
                        self.write(core::str::from_utf8(&buf).unwrap_or("?"));
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Raw REPL (machine protocol, used by mpremote and tools/mpy.py)
    // ------------------------------------------------------------------

    /// MicroPython's raw REPL: input isn't echoed; Ctrl-D runs it and the
    /// reply is `OK`, the output, `\x04`, the error text, `\x04`, `>`.
    /// The framing goes to the serial port only, never to a display.
    fn raw_mode(&mut self, store: &mut ScriptStore<'_>) -> Mode {
        print!("raw REPL; CTRL-B to exit\r\n>");
        self.arena.clear();
        let mut overflow = false;

        loop {
            match wait_byte() {
                CTRL_A => {
                    self.arena.clear();
                    print!("\r\nraw REPL; CTRL-B to exit\r\n>");
                }
                CTRL_B => {
                    self.write("\n");
                    return Mode::Normal;
                }
                CTRL_C => {
                    self.arena.clear();
                    overflow = false;
                }
                CTRL_D if self.arena.pending().is_empty() && !overflow => {
                    return Mode::SoftReset(true);
                }
                CTRL_D => {
                    print!("OK");
                    let result = if overflow {
                        self.arena.clear();
                        Err(Error::OutOfMemory)
                    } else {
                        self.run_raw(store)
                    };
                    overflow = false;
                    print!("\x04");
                    if let Err(err) = result {
                        self.report_error(err);
                    }
                    print!("\x04>");
                }
                CTRL_E if self.arena.pending().is_empty() => {
                    // Raw-paste mode request `\x05A\x01`: answer "understood but
                    // not supported", so the host falls back to plain raw mode.
                    if wait_byte() == b'A' && wait_byte() == CTRL_A {
                        print!("R\x00");
                    }
                }
                byte => {
                    if !self.arena.push(byte) {
                        overflow = true;
                    }
                }
            }
        }
    }

    /// Runs a raw-REPL program, or a `#@` command from `tools/mpy.py`.
    fn run_raw(&mut self, store: &mut ScriptStore<'_>) -> Result<(), Error> {
        let pending = self.arena.pending();

        if let Some(script) = pending.strip_prefix(SAVE_COMMAND.as_bytes()) {
            let len = script.len();
            let result = store.save(script);
            self.arena.clear();
            return match result {
                Ok(()) => {
                    let _ = write!(Fmt(self.vm.output()), "saved main.py ({} bytes)\n", len);
                    Ok(())
                }
                Err(crate::script_store::StoreError::TooLarge) => Err(Error::OutOfMemory),
                Err(_) => Err(Error::IoError),
            };
        }
        if pending.starts_with(ERASE_COMMAND.as_bytes()) {
            self.arena.clear();
            store.erase().map_err(|_| Error::IoError)?;
            self.write("main.py erased\n");
            return Ok(());
        }
        if pending.starts_with(CAT_COMMAND.as_bytes()) {
            self.arena.clear();
            // Print the saved script on the serial port only
            let free = self.arena.free_space();
            if let Some(len) = store.load(free) {
                for chunk in free[..len].utf8_chunks() {
                    print!("{}", chunk.valid());
                }
            }
            return Ok(());
        }

        let source = self.arena.commit()?;
        self.eval(source, false)
    }
}

/// True if a line ends with `:` (ignoring trailing spaces and a comment),
/// i.e. it starts an indented block.
fn ends_with_colon(line: &[u8]) -> bool {
    let code = match line.iter().position(|&b| b == b'#') {
        Some(hash) => &line[..hash],
        None => line,
    };
    code.iter().rev().find(|&&b| b != b' ') == Some(&b':')
}
