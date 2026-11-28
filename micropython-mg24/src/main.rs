//! micropython-rs on the Seeed XIAO MG24 (Silicon Labs EFR32MG24B220F1536IM48:
//! Arm Cortex-M33, 1536 KB flash, 256 KB RAM). The REPL is `micropython-port`'s;
//! this file gives it the console (`console`), the stack and the memory.
//!
//! Build and flash: `make flash` (probe-rs, through the board's CMSIS-DAP).
#![no_std]
#![no_main]

mod adc;
mod bench;
mod board;
mod clock;
mod console;
mod events;
mod gpio;
mod i2c;
mod imu;
mod pwm;
mod spi;
mod storage;
mod sysclock;
mod uart;

use core::cell::RefCell;
use core::mem::MaybeUninit;

use mg24_hal::clock::CpuSpeed;
use mg24_hal::gpio::{Input, InputConfig, Level, Output as Pin, OutputConfig};
use mg24_hal::CpuConfig;
use micropython_port::{BootScript, Memory, Port};
use micropython_rs::interpreter::FuncDef;
use micropython_rs::{Output, StmtCache};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("MPY_GIT_COMMIT");

// ---- the REPL's memory -----------------------------------------------------
//
// 256 KB of RAM: these statics take 136 KB, the stack (above them, up to the
// top of RAM) the rest. The linker script fails the build if they don't fit.

const ARENA_SIZE: usize = 32 * 1024;
const HEAP_SIZE: usize = 64 * 1024;
const MODULE_MEMORY_SIZE: usize = 16 * 1024;
const STMT_CACHE_SLOTS: usize = 16;
const FUNC_SLOTS: usize = 96;

static mut ARENA: [u8; ARENA_SIZE] = [0; ARENA_SIZE];
static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static mut MODULES: [u8; MODULE_MEMORY_SIZE] = [0; MODULE_MEMORY_SIZE];
static mut STMT_CACHE: [MaybeUninit<StmtCache<'static>>; STMT_CACHE_SLOTS] =
    [const { MaybeUninit::uninit() }; STMT_CACHE_SLOTS];
static mut FUNCS: [MaybeUninit<Option<FuncDef<'static>>>; FUNC_SLOTS] =
    [const { MaybeUninit::uninit() }; FUNC_SLOTS];

/// The memory for the REPL's sessions (called once, from `main`).
fn take_memory() -> Memory {
    // SAFETY: `main` calls this once, so these are the only references to
    // the statics
    unsafe {
        Memory {
            arena: &mut *core::ptr::addr_of_mut!(ARENA),
            heap: &mut *core::ptr::addr_of_mut!(HEAP),
            modules: &mut *core::ptr::addr_of_mut!(MODULES),
            stmt_cache: &mut *core::ptr::addr_of_mut!(STMT_CACHE),
            func_slots: &mut *core::ptr::addr_of_mut!(FUNCS),
            frozen: board::FROZEN_MODULES,
        }
    }
}

// ---- the stack ---------------------------------------------------------------

extern "C" {
    // From mg24-hal's linker script: the statics end at `__euninit`, the
    // stack grows down from `_stack_start` (the top of RAM) towards them
    static __euninit: u8;
    static _stack_start: u8;
}

/// The lowest stack address: the end of the statics.
fn stack_bottom() -> usize {
    // Only the symbol's address is taken (it's never read)
    core::ptr::addr_of!(__euninit) as usize
}

/// The main stack's size in bytes (`lscpu`).
pub fn stack_size() -> usize {
    // Only the symbols' addresses are taken
    core::ptr::addr_of!(_stack_start) as usize - stack_bottom()
}

/// Stack kept free below the interpreter's lowest check, for the code it
/// calls (formatting, flash writes) and for interrupts.
const STACK_MARGIN: usize = 8 * 1024;

// ---- the board's side of the REPL -------------------------------------------

struct Mg24Port;

impl Port for Mg24Port {
    const NAME: &'static str = "EFR32MG24 (XIAO MG24)";
    const VERSION: &'static str = VERSION;
    const COMMIT: &'static str = COMMIT;

    fn build_options(out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        let level = if cfg!(feature = "minimal") {
            "minimal"
        } else if cfg!(feature = "medium") {
            "medium"
        } else {
            "full"
        };
        let float = if cfg!(feature = "float32") { 32 } else { 64 };
        let mhz = sysclock::cpu_hz() / 1_000_000;
        write!(
            out,
            "chip=mg24 level={} features= float={} mhz={}",
            level, float, mhz
        )
    }

    fn read_byte() -> Option<u8> {
        console::read_byte()
    }

    fn discard_through(byte: u8) {
        console::discard_through(byte)
    }

    fn console_write(s: &str) {
        console::write_raw(s)
    }

    fn stack_floor() -> usize {
        stack_bottom() + STACK_MARGIN
    }
}

/// Script output and the REPL's prompts: on the console, with `\r\n`.
struct Console;

impl Output for Console {
    fn write_str(&mut self, s: &str) {
        console::write_crlf(s)
    }
}

static BOOT_SCRIPT: BootScript = BootScript {
    source: include_str!("../scripts/boot.py"),
    name: "scripts/boot.py",
};

/// The vector table in RAM (VTOR): exception entry reads a handler's address
/// from it, and the flash can't be read while a file's page is erased.
/// 2 + 14 + 70 words (the initial stack pointer, the reset vector, the core's
/// exceptions, the chip's interrupts), aligned to the next power of two.
const VECTOR_WORDS: usize = 2 + 14 + 70;
#[repr(C, align(512))]
struct Vectors([u32; VECTOR_WORDS]);
static mut RAM_VECTORS: Vectors = Vectors([0; VECTOR_WORDS]);

/// Copies the vector table (at the start of the flash) into RAM and points
/// SCB.VTOR at the copy. Before any interrupt is enabled.
fn vectors_to_ram() {
    // SAFETY: the table at 0x08000000 has VECTOR_WORDS words (mg24-hal's
    // link script checks its size); only this code writes RAM_VECTORS, once
    unsafe {
        let table = &mut *core::ptr::addr_of_mut!(RAM_VECTORS);
        for (i, word) in table.0.iter_mut().enumerate() {
            *word = (0x0800_0000 as *const u32).add(i).read_volatile();
        }
        core::ptr::write_volatile(0xE000_ED08 as *mut u32, table as *const _ as u32);
        core::arch::asm!("dsb", "isb", options(nomem, nostack, preserves_flags));
    }
}

#[mg24_hal::main]
fn main() -> ! {
    // A stack overflow faults (MSPLIM, ARMv8-M) instead of overwriting the
    // statics below the stack
    // SAFETY: the limit is below every frame in use (the statics' end)
    unsafe { core::arch::asm!("msr MSPLIM, {}", in(reg) stack_bottom()) };

    // 38 MHz from HFRCODPLL: twice the reset clock, and below the 40 MHz
    // where the flash needs a wait state and the 50 MHz PCLK limit (no
    // prescalers to change). 78 MHz needs the crystal and the DPLL.
    let mut config = CpuConfig::new();
    config.cpu_speed = Some(CpuSpeed::Mhz38);
    let dp = mg24_hal::init(config).unwrap();
    vectors_to_ram();
    // The console's TX (PA8) idle high, RX (PA9) an input; they stay
    // configured (`run` never returns)
    let _tx = Pin::new(dp.pins.pa8, Level::High, OutputConfig::default());
    let _rx = Input::new(dp.pins.pa9, InputConfig::new());
    // The 39 MHz crystal for the CPU, the timers and the EUSARTs (or
    // HFRCODPLL's 38 MHz if it doesn't start), before anything uses them
    sysclock::init(mg24_hal::clock::sysclk_hz());
    console::init(sysclock::pclk_hz());
    clock::init(sysclock::periph_hz());
    // The orange LED (PA7, lit when low) off
    let _ = gpio::pin_mode(7, gpio::OUT);
    gpio::write(gpio::PinId::new(7).unwrap(), true);

    let mut out = Console;
    let mut hw = board::Board::new();
    // main.py and other files, in the flash above the firmware
    let files = storage::mount().map(RefCell::new);
    micropython_port::run::<Mg24Port, _, _, _>(
        &mut out,
        &mut hw,
        &BOOT_SCRIPT,
        files.as_ref(),
        take_memory(),
        None,
    )
}

/// A fault (a bus fault on a peripheral without its clock, a stack overflow
/// past MSPLIM, ...) prints what it was on the console, then resets the
/// chip, instead of the HAL's default handler's silent loop.
#[unsafe(no_mangle)]
pub extern "C" fn HardFault() {
    // SCB.CFSR (configurable fault status), HFSR (hard fault status) and
    // BFAR (the address of a precise bus fault, when CFSR.BFARVALID)
    // SAFETY: reads of the System Control Block's fault registers
    let (cfsr, hfsr, bfar) = unsafe {
        (
            core::ptr::read_volatile(0xE000_ED28 as *const u32),
            core::ptr::read_volatile(0xE000_ED2C as *const u32),
            core::ptr::read_volatile(0xE000_ED38 as *const u32),
        )
    };
    let kind = if cfsr & 0x0010_0000 != 0 {
        "stack overflow (MSPLIM)"
    } else if cfsr & 0xFF00 != 0 {
        "bus fault"
    } else if cfsr & 0xFF != 0 {
        "memory fault"
    } else {
        "fault"
    };
    if cfsr & 0x8000 != 0 {
        panic!(
            "{} at {:#010x} (CFSR {:#010x}, HFSR {:#010x})",
            kind, bfar, cfsr, hfsr
        );
    }
    panic!("{} (CFSR {:#010x}, HFSR {:#010x})", kind, cfsr, hfsr);
}

/// A panic prints its message on the console, then resets the chip.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    struct Raw;
    impl core::fmt::Write for Raw {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            console::write_crlf(s);
            Ok(())
        }
    }
    mg24_hal::interrupt::disable();
    let _ = write!(Raw, "\n*** panic: {}\n*** resetting\n", info);
    console::flush();
    // SCB.AIRCR: VECTKEY and SYSRESETREQ
    // SAFETY: a write to the System Control Block's reset register
    unsafe { core::ptr::write_volatile(0xE000_ED0C as *mut u32, 0x05FA_0004) };
    loop {
        core::hint::spin_loop();
    }
}
