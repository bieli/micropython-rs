//! Events for MicroPython's `Pin.irq()` and `machine.Timer`: soft interrupts.
//!
//! A pin interrupt only records the pin (in `PENDING`, from the GPIO
//! interrupt handler); timers are software deadlines. The interpreter asks
//! for events between statements and while a script sleeps
//! (`HardwareBridge::poll_event`) and runs the Python callbacks then, like
//! MicroPython's soft IRQs (`hard=False`, its default). Event numbers: a pin
//! is its GPIO number (0-39), timer `i` is `TIMER_EVENT + i`.

use core::sync::atomic::{AtomicU32, Ordering};

use esp_hal::gpio::Event;

/// Pins whose interrupt fired and isn't handled yet: GPIO 0-31, 32-39.
static PENDING: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Event number of timer 0 (timer `i` is `TIMER_EVENT + i`).
pub const TIMER_EVENT: i64 = 100;
/// Timers: MicroPython's ESP32 hardware ids 0-3, and 4-7 for `Timer(-1)`.
pub const TIMERS: usize = 8;

/// The GPIO interrupt: records the pins that fired and clears them.
#[esp_hal::handler]
pub fn gpio_interrupt() {
    let gpio = esp_hal::peripherals::GPIO::regs();
    let low = gpio.status().read().bits();
    let high = gpio.status1().read().bits();
    gpio.status_w1tc().write(|w| unsafe { w.bits(low) });
    gpio.status1_w1tc().write(|w| unsafe { w.bits(high) });
    PENDING[0].fetch_or(low, Ordering::Relaxed);
    PENDING[1].fetch_or(high, Ordering::Relaxed);
}

/// MicroPython's `Pin.IRQ_RISING` (1), `IRQ_FALLING` (2), both (3).
pub fn trigger_event(trigger: i64) -> Option<Event> {
    match trigger {
        1 => Some(Event::RisingEdge),
        2 => Some(Event::FallingEdge),
        3 => Some(Event::AnyEdge),
        _ => None,
    }
}

/// Forgets a pin's interrupt that wasn't handled yet.
pub fn clear_pin(pin: u8) {
    PENDING[pin as usize / 32].fetch_and(!(1 << (pin % 32)), Ordering::Relaxed);
}

/// The next pin whose interrupt fired (and forgets it).
fn take_pin() -> Option<i64> {
    for (bank, pending) in PENDING.iter().enumerate() {
        let bits = pending.load(Ordering::Relaxed);
        if bits != 0 {
            let bit = bits.trailing_zeros();
            pending.fetch_and(!(1 << bit), Ordering::Relaxed);
            return Some((bank as u32 * 32 + bit) as i64);
        }
    }
    None
}

/// A software timer: `Timer.init(period=..., mode=...)`.
#[derive(Clone, Copy)]
pub struct SoftTimer {
    /// Microseconds since boot when it fires next.
    next_us: u64,
    period_us: u64,
    periodic: bool,
}

/// The board's events: timers, and whether sleeps stop for events.
pub struct Events {
    timers: [Option<SoftTimer>; TIMERS],
    /// The interpreter has a handler and isn't running it (`set_events`).
    pub on: bool,
}

impl Events {
    pub const fn new() -> Self {
        Self {
            timers: [None; TIMERS],
            on: false,
        }
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

    /// Stops every timer and forgets pending pins (a soft reset).
    pub fn reset(&mut self) {
        self.timers = [None; TIMERS];
        PENDING[0].store(0, Ordering::Relaxed);
        PENDING[1].store(0, Ordering::Relaxed);
    }

    /// An event is waiting (a sleep stops for it).
    pub fn waiting(&self, now_us: u64) -> bool {
        PENDING.iter().any(|p| p.load(Ordering::Relaxed) != 0)
            || self.timers.iter().flatten().any(|t| now_us >= t.next_us)
    }

    /// Any timer running (sleeps then check for events every millisecond).
    pub fn timers_running(&self) -> bool {
        self.timers.iter().any(Option::is_some)
    }

    /// The next event: a pin first, then a timer that's due (a periodic
    /// one is set to fire again; one that fell behind skips the missed
    /// periods instead of firing them all at once).
    pub fn poll(&mut self, now_us: u64) -> Option<i64> {
        if let Some(pin) = take_pin() {
            return Some(pin);
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
