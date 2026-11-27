//! The REPL on the ESP32 boards: `micropython-port`'s REPL (the normal REPL,
//! MicroPython's raw REPL, the `#@` file commands; see its docs for the keys)
//! with this firmware's console, stack, memory and frozen modules.

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;

use micropython_port::{Memory, Port};
use micropython_rs::interpreter::FuncDef;
use micropython_rs::{HardwareBridge, Output, StmtCache};

use crate::storage::Files;

pub use micropython_port::{BigHeap, BootScript};

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
static mut BUFFERS: MaybeUninit<Buffers> = MaybeUninit::uninit();

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
static mut STMT_CACHE: MaybeUninit<[StmtCache<'static>; STMT_CACHE_SLOTS]> = MaybeUninit::uninit();

/// The function table (`Interpreter::with_func_slots`): a slot for each
/// function, method and lambda alive at once, about 190 bytes each. Outside
/// the `Interpreter` struct, it no longer comes out of the stack three times
/// while the REPL starts (the struct is built, moved and kept). `machine`,
/// `network`, `time` and `ntptime` take 69. The S3 has room for 128 (59 left
/// for scripts) and still fits 8 nested calls with Wi-Fi, the interpreter's
/// limit; the C3 112 (43 left; at 128 its Wi-Fi build fit 7 calls, not 8).
/// The ESP32 keeps 96 (27 left): its stack next to the Wi-Fi driver fits 5.
const FUNC_SLOTS: usize = if cfg!(feature = "esp32s3") {
    128
} else if cfg!(feature = "esp32c3") {
    112
} else {
    96
};

static mut FUNCS: MaybeUninit<[Option<FuncDef<'static>>; FUNC_SLOTS]> = MaybeUninit::uninit();

/// The memory for the REPL's sessions; can be taken only once.
fn take_memory() -> Memory {
    static TAKEN: portable_atomic::AtomicBool = portable_atomic::AtomicBool::new(false);
    assert!(
        !TAKEN.swap(true, Ordering::Relaxed),
        "REPL memory taken twice"
    );
    // SAFETY: these are the only references to the statics (taken once,
    // checked above). The buffers are zeroed in place before they're used as
    // arrays of bytes (building `[0; N]` would use the stack); the REPL
    // writes every cache entry and function slot before it reads one
    unsafe {
        let buffers = core::ptr::addr_of_mut!(BUFFERS);
        core::ptr::write_bytes(buffers, 0, 1);
        let Buffers {
            arena,
            heap,
            modules,
        } = (*buffers).assume_init_mut();
        let stmt_cache = &mut *(core::ptr::addr_of_mut!(STMT_CACHE)
            as *mut [MaybeUninit<StmtCache<'static>>; STMT_CACHE_SLOTS]);
        let func_slots = &mut *(core::ptr::addr_of_mut!(FUNCS)
            as *mut [MaybeUninit<Option<FuncDef<'static>>>; FUNC_SLOTS]);
        Memory {
            arena,
            heap,
            modules,
            stmt_cache,
            func_slots,
            frozen: FROZEN_MODULES,
        }
    }
}

/// Stack kept free below the interpreter's lowest check, for the code it
/// calls (display drivers, flash writes, formatting) and for interrupts.
const STACK_MARGIN: usize = 16 * 1024;

/// This firmware's side of the REPL: the console (`serial_input`, printing
/// with esp-println), the stack (from esp-hal's linker script), its name and
/// build.
pub struct EspPort;

impl Port for EspPort {
    const NAME: &'static str = crate::chip::NAME;
    const VERSION: &'static str = crate::VERSION;
    const COMMIT: &'static str = crate::COMMIT;

    fn build_options(out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        crate::build_options(out)
    }

    fn read_byte() -> Option<u8> {
        crate::serial_input::read_byte()
    }

    fn wait_byte() -> u8 {
        crate::serial_input::wait_byte()
    }

    fn discard_through(byte: u8) {
        crate::serial_input::discard_through(byte)
    }

    fn console_write(s: &str) {
        esp_println::print!("{}", s);
        // The S3's and C3's USB-Serial/JTAG port can hold a short write back
        // (the raw REPL's closing `\x04>`, a `#@` command's answer): flush
        // once more while waiting for input, as for scripts' output
        crate::serial_input::output_written();
    }

    /// The bottom of the main stack plus `STACK_MARGIN`.
    fn stack_floor() -> usize {
        extern "C" {
            static _stack_end_cpu0: u8;
        }
        // Only the address of the linker symbol is taken (it's never read)
        core::ptr::addr_of!(_stack_end_cpu0) as usize + STACK_MARGIN
    }
}

/// Runs the boot script, then the REPL, forever. A soft reset starts a new
/// interpreter (all variables and functions are cleared).
pub fn run<O: Output, H: HardwareBridge>(
    out: &mut O,
    hw: &mut H,
    boot: &BootScript,
    files: Option<&RefCell<Files>>,
    big_heap: Option<BigHeap>,
) -> ! {
    micropython_port::run::<EspPort, _, _, _>(out, hw, boot, files, take_memory(), big_heap)
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
