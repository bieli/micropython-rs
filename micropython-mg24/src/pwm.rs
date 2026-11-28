//! PWM: TIMER1-TIMER4 (16 bits, 3 compare channels each: 12 pins). Pins with
//! the same frequency share a timer; a new frequency takes a free timer. Not
//! every timer reaches every port (the chip's routing): TIMER1 any port,
//! TIMER2 and TIMER4 ports A and B, TIMER3 ports C and D. So a pin on port
//! C/D (D0-D7, D18) has TIMER1 and TIMER3, one on port A/B (D8-D17) TIMER1,
//! TIMER2 and TIMER4. TIMER0 is the microsecond clock (clock.rs).
//!
//! The timers count the CPU clock divided by PRESC + 1 (chosen for the most
//! resolution: TOP up to 65535), in PWM mode: the output goes high when the
//! counter restarts and low at the channel's compare value, so
//! duty = OC / (TOP + 1).

use micropython_rs::Error;

use crate::gpio::{self, PinId};
use efr32mg24_pac as pac;

const TIMERS: usize = 4;
const CHANNELS: usize = 3;
/// A compare channel's registers take 8 words (CFG, CTRL, OC, -, OCB, ICF,
/// ICOF, -): channel `ch` is `0x20 * ch` above channel 0's.
const CC_STRIDE: usize = 0x20;

/// One timer in use: its frequency, TOP, and the pins on its channels.
#[derive(Clone, Copy)]
struct Slot {
    freq: u32,
    top: u32,
    pins: [Option<PinId>; CHANNELS],
}

pub struct Pwm {
    slots: [Option<Slot>; TIMERS],
    timer_hz: u32,
}

/// TIMER1-4's registers (they have the same layout as TIMER0's).
/// Whether TIMER(n + 1) can drive a pin on `port` (0 = A ... 3 = D).
fn reaches(n: usize, port: u8) -> bool {
    match n {
        0 => true,          // TIMER1
        1 | 3 => port <= 1, // TIMER2, TIMER4: ports A and B
        _ => port >= 2,     // TIMER3: ports C and D
    }
}

fn timer(n: usize) -> &'static pac::timer0_s::RegisterBlock {
    // SAFETY: TIMERn's registers; only this module uses TIMER1-4
    unsafe {
        let p = match n {
            0 => pac::Timer1S::ptr() as *const pac::timer0_s::RegisterBlock,
            1 => pac::Timer2S::ptr() as *const _,
            2 => pac::Timer3S::ptr() as *const _,
            _ => pac::Timer4S::ptr() as *const _,
        };
        &*p
    }
}

/// GPIO_TIMERn_ROUTEEN and the channel's CCxROUTE, as raw registers: the
/// PAC has one accessor per timer.
fn routes(n: usize, ch: usize) -> (*mut u32, *mut u32) {
    // SAFETY: GPIO's registers (only this module writes the TIMER routes)
    let g = unsafe { &*pac::GpioS::ptr() };
    let a = |r: *const u8| r as usize;
    let (en, cc0, cc1) = match n {
        0 => (
            a(g.timer1_routeen() as *const _ as _),
            a(g.timer1_cc0route() as *const _ as _),
            a(g.timer1_cc1route() as *const _ as _),
        ),
        1 => (
            a(g.timer2_routeen() as *const _ as _),
            a(g.timer2_cc0route() as *const _ as _),
            a(g.timer2_cc1route() as *const _ as _),
        ),
        2 => (
            a(g.timer3_routeen() as *const _ as _),
            a(g.timer3_cc0route() as *const _ as _),
            a(g.timer3_cc1route() as *const _ as _),
        ),
        _ => (
            a(g.timer4_routeen() as *const _ as _),
            a(g.timer4_cc0route() as *const _ as _),
            a(g.timer4_cc1route() as *const _ as _),
        ),
    };
    // CC1ROUTE and CC2ROUTE follow CC0ROUTE at the same spacing
    (en as *mut u32, (cc0 + (cc1 - cc0) * ch) as *mut u32)
}

impl Pwm {
    pub fn new(timer_hz: u32) -> Self {
        // SAFETY: CMU's clock enables for TIMER1-4
        unsafe {
            (&*pac::CmuS::ptr()).clken0().modify(|_, w| {
                w.timer1()
                    .set_bit()
                    .timer2()
                    .set_bit()
                    .timer3()
                    .set_bit()
                    .timer4()
                    .set_bit()
            })
        };
        Self {
            slots: [None; TIMERS],
            timer_hz,
        }
    }

    /// `pwm_u16(pin, freq, duty)`: duty 0-65535.
    pub fn set(&mut self, id: PinId, freq: u32, duty: u32) -> Result<(), Error> {
        if freq == 0 || freq > self.timer_hz / 2 || duty > 65535 {
            return Err(Error::ValueError);
        }
        // Already on a timer at this frequency: just the duty
        if let Some((n, ch)) = self.find(id) {
            let slot = self.slots[n].unwrap();
            if slot.freq == freq {
                self.write_duty(n, ch, slot.top, duty);
                return Ok(());
            }
            self.stop(id);
        }
        // A timer at this frequency with a free channel, or a free timer;
        // either way one that reaches the pin's port
        let (n, ch) = match (0..TIMERS).filter(|&n| reaches(n, id.port)).find_map(|n| {
            let s = self.slots[n]?;
            let ch = s.pins.iter().position(Option::is_none)?;
            (s.freq == freq).then_some((n, ch))
        }) {
            Some(found) => found,
            None => {
                let n = (0..TIMERS)
                    .find(|&n| reaches(n, id.port) && self.slots[n].is_none())
                    .ok_or(Error::ValueError)?;
                self.start_timer(n, freq);
                (n, 0)
            }
        };
        let mut slot = self.slots[n].unwrap();
        slot.pins[ch] = Some(id);
        self.slots[n] = Some(slot);
        gpio::set_push_pull(id, false);
        let t = timer(n);
        // SAFETY: TIMERn: a channel's mode can only change while it's disabled
        // (the other channels pause for this moment); the pin's route
        unsafe {
            t.en().write(|w| w.en().clear_bit());
            while t.en().read().bits() & 0b10 != 0 {} // DISABLING
            let cc_cfg = (t.cc0_cfg() as *const _ as usize + CC_STRIDE * ch) as *mut u32;
            cc_cfg.write_volatile(3); // MODE = PWM
            t.en().write(|w| w.en().set_bit());
            let (routeen, ccroute) = routes(n, ch);
            ccroute.write_volatile((id.pin as u32) << 16 | id.port as u32);
            routeen.write_volatile(routeen.read_volatile() | 1 << ch);
            t.cmd().write(|w| w.start().set_bit());
        }
        self.write_duty(n, ch, slot.top, duty);
        Ok(())
    }

    /// `pwm_stop(pin)`: the pin back to a low output.
    pub fn stop(&mut self, id: PinId) {
        let Some((n, ch)) = self.find(id) else { return };
        let (routeen, _) = routes(n, ch);
        // SAFETY: TIMERn's route enable for this channel
        unsafe { routeen.write_volatile(routeen.read_volatile() & !(1 << ch)) };
        gpio::write(id, false);
        let mut slot = self.slots[n].unwrap();
        slot.pins[ch] = None;
        if slot.pins.iter().all(Option::is_none) {
            timer(n).cmd().write(|w| w.stop().set_bit());
            self.slots[n] = None;
        } else {
            self.slots[n] = Some(slot);
        }
    }

    fn find(&self, id: PinId) -> Option<(usize, usize)> {
        (0..TIMERS).find_map(|n| {
            let s = self.slots[n]?;
            let ch = s.pins.iter().position(|p| *p == Some(id))?;
            Some((n, ch))
        })
    }

    /// Sets a free timer to `freq`: the smallest prescaler with TOP <= 65535.
    fn start_timer(&mut self, n: usize, freq: u32) {
        let presc = (self.timer_hz as u64)
            .div_ceil(freq as u64 * 65536)
            .clamp(1, 1024) as u32;
        let top = (self.timer_hz / presc / freq)
            .saturating_sub(1)
            .clamp(1, 65535);
        let t = timer(n);
        // SAFETY: TIMERn's configuration, written while it's disabled
        unsafe {
            t.en().write(|w| w.en().clear_bit());
            while t.en().read().bits() & 0b10 != 0 {}
            t.cfg().write(|w| w.presc().bits((presc - 1) as u16));
            t.en().write(|w| w.en().set_bit());
            t.top().write(|w| w.bits(top));
        }
        self.slots[n] = Some(Slot {
            freq,
            top,
            pins: [None; CHANNELS],
        });
    }

    fn write_duty(&self, n: usize, ch: usize, top: u32, duty: u32) {
        // OC = duty / 65536 of the period; 65535 is fully on (OC past TOP)
        let oc = if duty >= 65535 {
            top + 1
        } else {
            ((duty as u64 * (top as u64 + 1) + 32768) >> 16) as u32
        };
        let t = timer(n);
        // SAFETY: the channel's buffered compare value (taken at the next
        // period: no glitch)
        unsafe {
            let ocb = (t.cc0_ocb() as *const _ as usize + CC_STRIDE * ch) as *mut u32;
            ocb.write_volatile(oc);
        }
    }
}
