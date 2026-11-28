//! Time since boot and the wall clock.
//!
//! TIMER0 (32 bits) counts microseconds: its clock (EM01GRPACLK, which is
//! HFRCODPLL, the CPU's clock) divided down to 1 MHz. Its overflow interrupt
//! counts the wraps (every 71.6 minutes), so `now_us` is 64 bits and never
//! wraps in practice. The wall clock (`time.time()`, `machine.RTC`) is an
//! offset to it, set by `rtc_set`; it starts at 2000-01-01 00:00 UTC
//! (MicroPython's epoch on this port, as on the ESP32 boards).
//!
//! HFRCODPLL is an RC oscillator, calibrated in the factory to about 1-2 %:
//! the wall clock drifts by that much until the 39 MHz crystal is used.

use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering};

use efr32mg24_pac as pac;

/// The PAC's interrupt number of TIMER0.
const TIMER0_IRQ: u8 = 4;

/// TIMER0's wraps since boot (written by its interrupt).
static WRAPS: AtomicU32 = AtomicU32::new(0);
/// Wall clock minus `now_us`, in microseconds (`rtc_set`). Only the main
/// code uses it (no interrupt handler), and the chip has one core: a plain
/// cell (the Cortex-M33 has no 64-bit atomics).
struct MainOnly(Cell<i64>);
// SAFETY: see above: one core, never touched from an interrupt
unsafe impl Sync for MainOnly {}
static WALL_OFFSET_US: MainOnly = MainOnly(Cell::new(0));

#[inline(always)]
fn timer() -> &'static pac::timer0_s::RegisterBlock {
    // SAFETY: TIMER0's registers; after `init` only this module uses them
    unsafe { &*pac::Timer0S::ptr() }
}

/// Starts the microsecond counter; `timer_hz` is TIMER0's clock (the CPU
/// clock, a whole number of MHz).
pub fn init(timer_hz: u32) {
    // SAFETY: called once at boot, before the interrupt is enabled
    unsafe {
        let cmu = &*pac::CmuS::ptr();
        cmu.clken0().modify(|_, w| w.timer0().set_bit());
        let t = timer();
        // CFG can only be written while the timer is disabled
        t.en().write(|w| w.en().clear_bit());
        let presc = timer_hz / 1_000_000 - 1;
        t.cfg().write(|w| w.presc().bits(presc as u16));
        t.en().write(|w| w.en().set_bit());
        t.top().write(|w| w.bits(u32::MAX));
        t.ien().write(|w| w.of().set_bit());
        t.cmd().write(|w| w.start().set_bit());
        mg24_hal::interrupt::enable_irq(TIMER0_IRQ);
    }
}

/// TIMER0's overflow: one more wrap. From RAM, as the console's handler.
#[unsafe(no_mangle)]
#[link_section = ".ram_text.timer0"]
pub extern "C" fn TIMER0() {
    let t = timer();
    if t.if_().read().of().bit_is_set() {
        t.if_().modify(|_, w| w.of().clear_bit());
        WRAPS.fetch_add(1, Ordering::Release);
    }
}

/// Microseconds since boot.
pub fn now_us() -> u64 {
    let t = timer();
    loop {
        let wraps = WRAPS.load(Ordering::Acquire);
        let count = t.cnt().read().bits();
        if wraps != WRAPS.load(Ordering::Acquire) {
            continue; // the interrupt counted a wrap meanwhile: read again
        }
        // A wrap whose interrupt can't run yet (interrupts are off, or this
        // is an interrupt handler): the counter has just restarted from 0
        let pending = t.if_().read().of().bit_is_set() && count < 1 << 31;
        return ((wraps as u64 + pending as u64) << 32) | count as u64;
    }
}

/// Milliseconds since boot.
pub fn now_ms() -> u64 {
    now_us() / 1000
}

/// Waits `us` microseconds (busy).
pub fn delay_us(us: u64) {
    let until = now_us().saturating_add(us);
    while now_us() < until {}
}

/// Microseconds since 2000-01-01 00:00 UTC.
pub fn wall_us() -> i64 {
    now_us() as i64 + WALL_OFFSET_US.0.get()
}

/// Sets the wall clock (`machine.RTC().datetime(...)`).
pub fn set_wall_us(us: i64) {
    WALL_OFFSET_US.0.set(us - now_us() as i64);
}
