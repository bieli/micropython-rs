//! Events for MicroPython's `Pin.irq()` and `machine.Timer`: soft interrupts,
//! as on the ESP32 boards (micropython-esp32's events.rs, with this chip's
//! GPIO interrupts).
//!
//! A pin interrupt only records its line (from the GPIO interrupt handlers,
//! in RAM like the console's); timers are software deadlines. The
//! interpreter asks for events between statements and while a script sleeps
//! (`HardwareBridge::poll_event`) and runs the Python callbacks then.
//! Event numbers: a pin is its number (`port * 16 + pin`), timer `i` is
//! `TIMER_EVENT + i`.
//!
//! The chip has 12 external interrupt lines; line `n` can take pin `n` (in
//! its group of 4 pin numbers) of any port. Here pin number `n` uses line `n`,
//! so two pins with the same number on different ports (PA3, PC3) can't
//! both have an interrupt: the second one is a `ValueError`.

use core::sync::atomic::{AtomicU32, Ordering};

use micropython_rs::Error;

use crate::gpio::PinId;
use efr32mg24_pac as pac;

/// Event number of timer 0 (timer `i` is `TIMER_EVENT + i`).
pub const TIMER_EVENT: i64 = 100;
/// Timers: ids 0-3, and 4 for `Timer(-1)`, as on the ESP32 boards.
pub const TIMERS: usize = 8;
const LINES: usize = 12;
const GPIO_ODD_IRQ: u8 = 25;
const GPIO_EVEN_IRQ: u8 = 26;

/// Lines whose interrupt fired and isn't handled yet (bit per line).
static PENDING: AtomicU32 = AtomicU32::new(0);

#[inline(always)]
fn gpio() -> &'static pac::gpio_s::RegisterBlock {
    // SAFETY: GPIO's registers; the interrupt handlers only read IF and
    // write IF_CLR, the main code owns the rest
    unsafe { &*pac::GpioS::ptr() }
}

/// The GPIO interrupts (even and odd lines): record the lines and clear them
/// (through IF's CLR alias). From RAM: they fire while a file is saved too.
#[inline(always)]
fn on_gpio_irq() {
    let g = gpio();
    let fired = g.if_().read().bits() & g.ien().read().bits() & ((1 << LINES) - 1);
    let clr = (g.if_() as *const _ as usize + 0x2000) as *mut u32;
    // SAFETY: GPIO_IF_CLR
    unsafe { clr.write_volatile(fired) };
    PENDING.fetch_or(fired, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
#[link_section = ".ram_text.gpio_even"]
pub extern "C" fn GPIO_EVEN() {
    on_gpio_irq();
}

#[unsafe(no_mangle)]
#[link_section = ".ram_text.gpio_odd"]
pub extern "C" fn GPIO_ODD() {
    on_gpio_irq();
}

/// A software timer: `Timer.init(period=..., mode=...)`.
#[derive(Clone, Copy)]
struct SoftTimer {
    /// Microseconds since boot when it fires next.
    next_us: u64,
    period_us: u64,
    periodic: bool,
}

/// The board's events: pin lines in use, timers, and whether sleeps stop
/// for events.
pub struct Events {
    /// The pin (`port * 16 + pin`) on each line, if it has an interrupt.
    lines: [Option<PinId>; LINES],
    timers: [Option<SoftTimer>; TIMERS],
    /// The interpreter has a handler and isn't running it (`set_events`).
    pub on: bool,
}

impl Events {
    pub fn new() -> Self {
        // SAFETY: called once at boot, before the GPIO interrupts are enabled
        unsafe {
            mg24_hal::interrupt::enable_irq(GPIO_EVEN_IRQ);
            mg24_hal::interrupt::enable_irq(GPIO_ODD_IRQ);
        }
        Self {
            lines: [None; LINES],
            timers: [None; TIMERS],
            on: false,
        }
    }

    /// `pin_irq(pin, trigger)`: 1 rising, 2 falling, 3 both, 0 off.
    pub fn pin_irq(&mut self, id: PinId, trigger: i64) -> Result<(), Error> {
        let line = id.pin as usize;
        if line >= LINES || !(0..=3).contains(&trigger) {
            return Err(Error::ValueError);
        }
        if matches!(self.lines[line], Some(other) if other != id) && trigger != 0 {
            return Err(Error::ValueError); // its line serves the same pin number on another port
        }
        let g = gpio();
        let bit = 1u32 << line;
        // SAFETY: GPIO's EXTI registers; only this function changes them
        unsafe {
            g.ien().modify(|r, w| w.bits(r.bits() & !bit));
            if trigger == 0 {
                if self.lines[line] == Some(id) {
                    self.lines[line] = None;
                }
                PENDING.fetch_and(!bit, Ordering::Relaxed);
                return Ok(());
            }
            // The line's port (4 bits per line) and its pin in the group of 4 (2 bits)
            let (sel, pinsel) = if line < 8 {
                (
                    g.extipsell() as *const _ as *mut u32,
                    g.extipinsell() as *const _ as *mut u32,
                )
            } else {
                (
                    g.extipselh() as *const _ as *mut u32,
                    g.extipinselh() as *const _ as *mut u32,
                )
            };
            let n = (line % 8) as u32;
            sel.write_volatile(
                sel.read_volatile() & !(0xF << (4 * n)) | (id.port as u32) << (4 * n),
            );
            pinsel.write_volatile(
                pinsel.read_volatile() & !(0x3 << (4 * n)) | (line as u32 % 4) << (4 * n),
            );
            let rise = g.extirise() as *const _ as *mut u32;
            let fall = g.extifall() as *const _ as *mut u32;
            let set = |reg: *mut u32, on: bool| {
                reg.write_volatile(if on {
                    reg.read_volatile() | bit
                } else {
                    reg.read_volatile() & !bit
                })
            };
            set(rise, trigger & 1 != 0);
            set(fall, trigger & 2 != 0);
            ((g.if_() as *const _ as usize + 0x2000) as *mut u32).write_volatile(bit);
            PENDING.fetch_and(!bit, Ordering::Relaxed);
            self.lines[line] = Some(id);
            g.ien().modify(|r, w| w.bits(r.bits() | bit));
        }
        Ok(())
    }

    pub fn start_timer(&mut self, slot: usize, period_us: u64, periodic: bool, now_us: u64) {
        self.timers[slot] = Some(SoftTimer {
            next_us: now_us + period_us,
            period_us,
            periodic,
        });
    }

    pub fn stop_timer(&mut self, slot: usize) {
        self.timers[slot] = None;
    }

    /// Stops every timer and pin interrupt (a soft reset).
    pub fn reset(&mut self) {
        // SAFETY: GPIO's interrupt enables
        unsafe {
            gpio()
                .ien()
                .modify(|r, w| w.bits(r.bits() & !((1 << LINES) - 1)))
        };
        PENDING.store(0, Ordering::Relaxed);
        self.lines = [None; LINES];
        self.timers = [None; TIMERS];
    }

    /// An event is waiting (a sleep stops for it).
    pub fn waiting(&self, now_us: u64) -> bool {
        PENDING.load(Ordering::Relaxed) != 0
            || self.timers.iter().flatten().any(|t| now_us >= t.next_us)
    }

    /// The next event: a pin first, then a timer that's due (a periodic one
    /// is set to fire again; one that fell behind skips the missed periods
    /// instead of firing them all at once).
    pub fn poll(&mut self, now_us: u64) -> Option<i64> {
        let pending = PENDING.load(Ordering::Relaxed);
        if pending != 0 {
            let line = pending.trailing_zeros() as usize;
            PENDING.fetch_and(!(1 << line), Ordering::Relaxed);
            if let Some(id) = self.lines[line] {
                return Some(id.port as i64 * 16 + id.pin as i64);
            }
        }
        for (i, slot) in self.timers.iter_mut().enumerate() {
            let Some(timer) = slot else { continue };
            if now_us < timer.next_us {
                continue;
            }
            if timer.periodic {
                timer.next_us += timer.period_us;
                if timer.next_us <= now_us {
                    timer.next_us = now_us + timer.period_us;
                }
            } else {
                *slot = None;
            }
            return Some(TIMER_EVENT + i as i64);
        }
        None
    }
}

/// MicroPython's timer ids: 0-3 and -1 (slot 4), as on the ESP32 boards.
pub fn timer_slot(id: i64) -> Result<usize, Error> {
    match id {
        0..=3 => Ok(id as usize),
        -1 => Ok(4),
        _ => Err(Error::ValueError),
    }
}
