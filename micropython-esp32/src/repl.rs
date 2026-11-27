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
use micropython_rs::{Error, HardwareBridge, Interpreter, Output, StmtCache};

use micropython_rs::flatfs::{FsError, Writer};

use crate::serial_input::wait_byte;
use crate::storage::Files;
use crate::ABORT;
use core::cell::RefCell;

/// Bytes of REPL input (and the boot script) kept per session.
pub const ARENA_SIZE: usize = 32 * 1024;
/// Interpreter heap for objects created at runtime (strings built by scripts).
pub const HEAP_SIZE: usize = 32 * 1024;

/// Source text of the `.py` modules scripts import (`import sensors` reads
/// `sensors.py` from flash into it); emptied by every soft reset.
pub const MODULE_MEMORY_SIZE: usize = 16 * 1024;

/// The REPL's big buffers (80 KB). On the ESP32 they live in the second data
/// RAM region (`dram2`, 96 KB the bootloader leaves free), not in the main
/// data RAM: there, they would come out of the stack, which takes all the
/// main data RAM that statics leave. The ESP32-S3's `dram2` is smaller (74 KB),
/// and its main RAM bigger: there they are ordinary statics.
struct Buffers {
    arena: [u8; ARENA_SIZE],
    heap: [u8; HEAP_SIZE],
    modules: [u8; MODULE_MEMORY_SIZE],
}

#[cfg_attr(feature = "esp32", link_section = ".dram2_uninit")]
static mut BUFFERS: core::mem::MaybeUninit<Buffers> = core::mem::MaybeUninit::uninit();

/// The buffers, zeroed; can be taken only once.
fn take_buffers() -> &'static mut Buffers {
    static TAKEN: portable_atomic::AtomicBool = portable_atomic::AtomicBool::new(false);
    assert!(
        !TAKEN.swap(true, Ordering::Relaxed),
        "REPL buffers taken twice"
    );
    // SAFETY: this is the only reference to BUFFERS (taken once, checked
    // above), and the memory is zeroed in place before it's used as arrays
    // of bytes (zeroed in place: building `[0; N]` would use the stack)
    unsafe {
        let buffers = core::ptr::addr_of_mut!(BUFFERS);
        core::ptr::write_bytes(buffers, 0, 1);
        (*buffers).assume_init_mut()
    }
}

/// Parsed statements kept for loops and functions (`Interpreter::with_stmt_cache`):
/// about 0.5 KB each. The ESP32 with Wi-Fi has the least RAM to spare (its
/// `dram2` holds the driver's heap): 8 there, 16 elsewhere.
const STMT_CACHE_SLOTS: usize = if cfg!(all(feature = "esp32", feature = "wifi")) {
    8
} else {
    16
};

/// The ESP32 without Wi-Fi keeps it in `dram2` (16 KB are left there).
#[cfg_attr(
    all(feature = "esp32", not(feature = "wifi")),
    link_section = ".dram2_uninit"
)]
static mut STMT_CACHE: core::mem::MaybeUninit<[StmtCache<'static>; STMT_CACHE_SLOTS]> =
    core::mem::MaybeUninit::uninit();

/// The statement cache for one session, every entry empty.
///
/// # Safety
/// Only one session may hold it at a time (`run` makes a new one for each).
unsafe fn take_stmt_cache<'a>() -> &'a mut [StmtCache<'a>] {
    let cache = core::ptr::addr_of_mut!(STMT_CACHE) as *mut StmtCache<'a>;
    for i in 0..STMT_CACHE_SLOTS {
        // SAFETY: inside the static; written whole before any use
        unsafe { cache.add(i).write(StmtCache::EMPTY) };
    }
    // SAFETY: just initialised; the caller holds the only reference. The
    // entries only ever point into this session's source text, and the
    // interpreter clears them at every `eval`
    unsafe { core::slice::from_raw_parts_mut(cache, STMT_CACHE_SLOTS) }
}

const CTRL_A: u8 = 0x01;
const CTRL_B: u8 = 0x02;
const CTRL_C: u8 = 0x03;
const CTRL_D: u8 = 0x04;
const CTRL_E: u8 = 0x05;
const BACKSPACE: u8 = 0x08;
const TAB: u8 = 0x09;
const ESC: u8 = 0x1B;
const DELETE: u8 = 0x7F;

/// The file run at boot, and by `#@save` / `#@cat` / `#@erase`.
const MAIN_FILE: &str = "main.py";
/// Data bytes per `#@get` output chunk.
const GET_CHUNK: usize = 256;

/// Stack kept free below the interpreter's lowest check, for the code it
/// calls (display drivers, flash writes, formatting) and for interrupts.
const STACK_MARGIN: usize = 16 * 1024;

/// The lowest stack address scripts may use: the bottom of the main stack
/// (from esp-hal's linker script) plus `STACK_MARGIN`.
fn stack_floor() -> usize {
    extern "C" {
        static _stack_end_cpu0: u8;
    }
    // Only the address of the linker symbol is taken (it's never read)
    core::ptr::addr_of!(_stack_end_cpu0) as usize + STACK_MARGIN
}

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
    files: Option<&RefCell<Files>>,
) -> ! {
    let Buffers {
        arena: memory,
        heap,
        modules: module_memory,
    } = take_buffers();
    let mut mode = Mode::Normal;
    let mut run_boot_script = true;

    loop {
        // Like MicroPython: a soft reset stops the last session's pin
        // interrupts and timers
        hw.soft_reset();
        // SAFETY: the previous session's interpreter (the only other user)
        // is gone
        let stmt_cache = unsafe { take_stmt_cache() };
        let mut vm = new_interpreter(out, hw, heap, files, module_memory, stmt_cache);
        let mut repl = Repl {
            vm: &mut vm,
            arena: Arena::new(&mut memory[..]),
            history: None,
        };

        // Like MicroPython, a soft reset in the raw REPL skips main.py
        if run_boot_script {
            repl.run_boot_script(boot, files);
        }

        loop {
            mode = match mode {
                Mode::Normal => repl.normal_mode(),
                Mode::Raw => repl.raw_mode(files),
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

/// The largest frozen module's source (in flash): what `machine.bench()`'s
/// flash read test reads.
pub fn frozen_text() -> &'static str {
    FROZEN_MODULES
        .iter()
        .map(|&(_, text)| text)
        .max_by_key(|text| text.len())
        .unwrap_or("")
}

/// Python modules built into the firmware (frozen): imported like files,
/// without using the module memory. A file of the same name comes first.
static FROZEN_MODULES: &[(&str, &str)] = &[
    #[cfg(feature = "machine")]
    ("machine", include_str!("../modules/machine.py")),
    // On top of the native `utime` (board.rs), with the calendar functions
    #[cfg(feature = "machine")]
    ("time", include_str!("../modules/time.py")),
    #[cfg(feature = "wifi")]
    ("network", include_str!("../modules/network.py")),
    #[cfg(all(feature = "wifi", feature = "machine"))]
    ("ntptime", include_str!("../modules/ntptime.py")),
];

/// A new interpreter for a session. Out of line: building it takes a
/// temporary copy of its 27 KB, which would otherwise stay in `run`'s frame,
/// under every script, for good (here it's freed before the session runs).
/// The REPL then only borrows it, so it isn't moved again.
#[inline(never)]
fn new_interpreter<'a, O: Output, H: HardwareBridge>(
    out: &'a mut O,
    hw: &'a mut H,
    heap: &'a mut [u8],
    files: Option<&'a RefCell<Files>>,
    module_memory: &'a mut [u8],
    stmt_cache: &'a mut [StmtCache<'a>],
) -> Interpreter<'a, O, H> {
    let mut vm = Interpreter::with_hardware(out, hw);
    vm.set_heap(heap);
    vm.set_stmt_cache(stmt_cache);
    // Scripts get the flash files too: `open()` and the `os` module
    if let Some(files) = files {
        vm.set_files(files);
        vm.set_module_memory(module_memory);
    }
    vm.set_frozen_modules(FROZEN_MODULES);
    vm.set_abort_flag(&ABORT);
    // Deep recursion raises RecursionError instead of overflowing the stack
    vm.set_stack_floor(Some(stack_floor()));
    vm
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

struct Repl<'r, 'a, O: Output, H: HardwareBridge> {
    vm: &'r mut Interpreter<'a, O, H>,
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

impl<'a, O: Output, H: HardwareBridge> Repl<'_, 'a, O, H> {
    fn write(&mut self, s: &str) {
        self.vm.output().write_str(s);
    }

    fn run_boot_script(&mut self, boot: &BootScript, files: Option<&RefCell<Files>>) {
        // Borrowed only to read main.py: the script may use the files itself
        let saved = files.map(|fs| fs.borrow_mut().read(MAIN_FILE, self.arena.free_space()));
        let source = match saved {
            Some(Ok(len)) => {
                self.arena.pending = len;
                match self.arena.commit() {
                    Ok(source) => {
                        let _ = write!(
                            Fmt(self.vm.output()),
                            "main.py from flash ({} bytes); built-in {} not used\n",
                            len,
                            boot.name
                        );
                        source
                    }
                    Err(_) => {
                        self.write("main.py is not valid UTF-8, skipped\n");
                        return;
                    }
                }
            }
            Some(Err(err)) if err != FsError::NotFound => {
                let _ = write!(Fmt(self.vm.output()), "main.py: {}\n", err.message());
                return;
            }
            _ => {
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

    /// Prints a Python-style traceback (Thonny and mpremote show it as is).
    fn report_error(&mut self, err: Error) {
        self.vm.print_traceback(err, "<stdin>");
        self.write("\n");
    }

    fn banner(&mut self) {
        self.write("\nmicropython-rs REPL on ");
        self.write(crate::chip::NAME);
        self.write(
            "\nhelp() lists functions. Ctrl-C stops a script, Ctrl-D soft resets, Ctrl-E pastes.\n",
        );
    }

    // ------------------------------------------------------------------
    // Normal REPL
    // ------------------------------------------------------------------

    /// Waits for a byte at the `>>>` prompt, running the callbacks of pin
    /// interrupts and timers meanwhile (a `Timer` keeps going after its
    /// script, as in MicroPython). Not in the raw REPL: tools like Thonny
    /// read its output as their protocol.
    fn wait_byte_idle(&mut self) -> u8 {
        loop {
            if let Some(byte) = crate::serial_input::read_byte() {
                return byte;
            }
            // Ctrl-C arrives as a byte too, so errors end here
            let _ = self.vm.idle();
        }
    }

    /// Waits for a byte in the raw REPL (Thonny, mpremote): the network
    /// goes on meanwhile (the access point's DHCP server), but no callbacks
    /// run, as their output would get into the tool's protocol.
    fn wait_byte_raw(&mut self) -> u8 {
        loop {
            if let Some(byte) = crate::serial_input::read_byte() {
                return byte;
            }
            self.vm.idle_hardware();
        }
    }

    fn normal_mode(&mut self) -> Mode {
        self.banner();
        self.write(">>> ");
        self.arena.clear();

        let mut line_start = 0; // where the line being typed starts in `pending`
        let mut in_block = false; // collecting an indented block (`... ` prompt)
        let mut last_was_cr = false;
        let mut escape = 0u8; // progress through an `ESC [ x` key sequence

        loop {
            let byte = self.wait_byte_idle();
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
        // `x = [1,` continues on the next line until the brackets are closed
        let open_brackets = brackets_open(self.arena.pending());

        if open_brackets || in_block && !blank || !in_block && opens_block {
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
    fn raw_mode(&mut self, files: Option<&RefCell<Files>>) -> Mode {
        print!("raw REPL; CTRL-B to exit\r\n>");
        self.arena.clear();
        let mut overflow = false;
        // A file being uploaded with #@open / #@write / #@close
        let mut upload: Option<Writer> = None;

        loop {
            match self.wait_byte_raw() {
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
                    // Like MicroPython: the empty command is acknowledged first
                    print!("OK\r\n");
                    return Mode::SoftReset(true);
                }
                CTRL_D => {
                    print!("OK");
                    let result = if overflow {
                        self.arena.clear();
                        Err(RawError::Python(Error::OutOfMemory))
                    } else {
                        self.run_raw(files, &mut upload)
                    };
                    overflow = false;
                    print!("\x04");
                    match result {
                        Ok(()) => {}
                        Err(RawError::Python(err)) => self.report_error(err),
                        Err(RawError::Fs(err)) => {
                            let _ = write!(Fmt(self.vm.output()), "OSError: {}\n", err.message());
                        }
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

    /// Runs a raw-REPL program, or a `#@` file command from `tools/mpy.py`.
    fn run_raw(
        &mut self,
        files: Option<&RefCell<Files>>,
        upload: &mut Option<Writer>,
    ) -> Result<(), RawError> {
        if !self.arena.pending().starts_with(b"#@") {
            let source = self.arena.commit().map_err(RawError::Python)?;
            return self.eval(source, false).map_err(RawError::Python);
        }
        // A file command: no script runs meanwhile, so the files can be borrowed
        let mut files = files.map(|fs| fs.borrow_mut());
        let result = self.file_command(files.as_deref_mut(), upload);
        self.arena.clear();
        result
    }

    /// `#@` commands. Names and hex data follow the command on its line;
    /// `#@save` has the file text on the following lines.
    fn file_command(
        &mut self,
        files: Option<&mut Files>,
        upload: &mut Option<Writer>,
    ) -> Result<(), RawError> {
        let fs = files.ok_or(RawError::Fs(FsError::NoRegion))?;
        let pending = self.arena.pending();
        let (line, rest) = match pending.iter().position(|&b| b == b'\n') {
            Some(i) => (&pending[..i], &pending[i + 1..]),
            None => (pending, &[][..]),
        };
        let line = core::str::from_utf8(line).map_err(|_| RawError::Fs(FsError::BadName))?;
        let (command, arg) = line.split_once(' ').unwrap_or((line, ""));
        let arg = arg.trim();

        match command {
            // tools/mpy.py save: the text after the first line becomes main.py
            "#@save" => {
                fs.write(MAIN_FILE, rest)?;
                let len = rest.len();
                let _ = write!(Fmt(self.vm.output()), "saved main.py ({} bytes)\n", len);
            }
            "#@cat" => {
                let file = fs.find(MAIN_FILE)?.ok_or(FsError::NotFound)?;
                stream_file(fs, &file, false)?;
            }
            "#@erase" => {
                fs.remove(MAIN_FILE)?;
                self.write("main.py erased\n");
            }
            "#@ls" => {
                let (used, total) = fs.usage()?;
                fs.list(|f| print!("{} {}\r\n", f.name(), f.size))?;
                print!(
                    "# {} of {} files, max {} bytes each\r\n",
                    used,
                    total,
                    micropython_rs::flatfs::MAX_FILE_SIZE
                );
            }
            "#@get" => {
                let file = fs.find(arg)?.ok_or(FsError::NotFound)?;
                stream_file(fs, &file, true)?;
            }
            "#@rm" => fs.remove(arg)?,
            "#@open" => *upload = Some(fs.begin(arg)?),
            "#@write" => {
                let writer = upload.as_mut().ok_or(FsError::NotFound)?;
                let mut chunk = [0u8; 128];
                for pair in arg.as_bytes().chunks(2 * chunk.len()) {
                    let n = hex_decode(pair, &mut chunk).ok_or(FsError::BadName)?;
                    fs.append(writer, &chunk[..n])?;
                }
            }
            "#@close" => {
                let writer = upload.take().ok_or(FsError::NotFound)?;
                let size = writer.size();
                fs.finish(writer)?;
                print!("{}", size);
            }
            _ => return Err(RawError::Python(Error::InvalidSyntax)),
        }
        Ok(())
    }
}

/// Result of a raw-REPL request: a script error or a file system error.
enum RawError {
    Python(Error),
    Fs(FsError),
}

impl From<FsError> for RawError {
    fn from(err: FsError) -> Self {
        RawError::Fs(err)
    }
}

/// Prints a file on the serial port: as hex (binary-safe, for `#@get`) or as text.
fn stream_file(
    fs: &mut Files,
    file: &micropython_rs::flatfs::FileInfo,
    hex: bool,
) -> Result<(), FsError> {
    let mut buf = [0u8; GET_CHUNK];
    let mut offset = 0;
    loop {
        let n = fs.read_at(file, offset, &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        if hex {
            let mut text = [0u8; 2 * GET_CHUNK];
            for (i, b) in buf[..n].iter().enumerate() {
                text[2 * i] = HEX[(b >> 4) as usize];
                text[2 * i + 1] = HEX[(b & 0xF) as usize];
            }
            print!("{}", core::str::from_utf8(&text[..2 * n]).unwrap_or(""));
        } else {
            for chunk in buf[..n].utf8_chunks() {
                print!("{}", chunk.valid());
            }
        }
        offset += n as u32;
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Decodes hex digit pairs into `out`; `None` for an odd length or a non-hex character.
fn hex_decode(hex: &[u8], out: &mut [u8]) -> Option<usize> {
    if hex.len() % 2 != 0 || hex.len() / 2 > out.len() {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    for (i, pair) in hex.chunks(2).enumerate() {
        out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(hex.len() / 2)
}

/// True if a line ends with `:` (ignoring trailing spaces and a comment),
/// i.e. it starts an indented block.
/// True if `code` has more opening than closing brackets (outside strings
/// and comments).
fn brackets_open(code: &[u8]) -> bool {
    let mut depth = 0i32;
    let mut quote = None;
    let mut iter = code.iter();
    while let Some(&b) = iter.next() {
        match (quote, b) {
            (Some(_), b'\\') => {
                iter.next();
            }
            (Some(q), _) if b == q => quote = None,
            (Some(_), b'\n') => quote = None, // unterminated: the parser reports it
            (Some(_), _) => {}
            (None, b'\'' | b'"') => quote = Some(b),
            (None, b'#') => {
                for &c in iter.by_ref() {
                    if c == b'\n' {
                        break;
                    }
                }
            }
            (None, b'(' | b'[' | b'{') => depth += 1,
            (None, b')' | b']' | b'}') => depth -= 1,
            _ => {}
        }
    }
    depth > 0
}

fn ends_with_colon(line: &[u8]) -> bool {
    let code = match line.iter().position(|&b| b == b'#') {
        Some(hash) => &line[..hash],
        None => line,
    };
    code.iter().rev().find(|&&b| b != b' ') == Some(&b':')
}
