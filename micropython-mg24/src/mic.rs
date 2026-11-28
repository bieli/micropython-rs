//! The XIAO MG24 Sense's microphone, an MSM381ACT001 (analog MEMS, with an
//! amplifier): its output on PC9, its power from PC8 (high: on; from the
//! board's schematic and Seeed's examples). Measured on the board: the
//! output sits at a 0.57 V bias with room noise of about 5-7 mV rms; with
//! PC8 low it reads a flat 0.19 V. After power-on the bias starts near 1 V
//! and settles in about 250 ms (within 5 mV: 641 mV at 80 ms, 576 at 155,
//! 566 at 230).
//!
//! The module itself is `micropython_port::mic` (any analog microphone, on
//! any ADC pin: `mic.init(pin, power, rate)`); this is the board's part:
//! power, an analog input, and IADC0's timer-triggered sampling.

use micropython_port::mic::{self, MicHw};
use micropython_rs::{Error, Module};

use crate::adc::Adc;
use crate::gpio::{self, PinId};

/// PC9 (the signal) and PC8 (its power).
pub const PIN: i64 = 2 * 16 + 9;
pub const POWER: i64 = 2 * 16 + 8;
pub const RATE: u32 = 16_000;
/// Power-on to a settled bias, ms (measured, see above).
pub const SETTLE_MS: u32 = 250;

pub const MODULE: Module = Module {
    name: "mic",
    doc: "An analog microphone (the Sense's MSM381 on PC9, powered from PC8)",
    functions: mic::FUNCTIONS,
    constants: &[("PIN", PIN), ("POWER", POWER)],
};

/// The board's side of `mic`, on the ADC.
pub struct Hw<'a> {
    pub adc: &'a mut Adc,
}

impl MicHw for Hw<'_> {
    fn power(&mut self, pin: i64, on: bool) -> Result<(), Error> {
        gpio::pin_mode(pin, gpio::OUT)?;
        gpio::write(PinId::new(pin)?, on);
        Ok(())
    }

    fn analog_input(&mut self, pin: i64) -> Result<(), Error> {
        gpio::release(PinId::new(pin)?); // DISABLED: no digital input or pull
        Ok(())
    }

    fn actual_rate(&mut self, rate_hz: u32) -> Result<f64, Error> {
        self.adc.actual_rate(rate_hz)
    }

    fn sample(
        &mut self,
        pin: i64,
        rate_hz: u32,
        n: usize,
        each: &mut dyn FnMut(u16),
    ) -> Result<(), Error> {
        self.adc.sample(PinId::new(pin)?, rate_hz, n, each)
    }

    fn full_scale(&mut self) -> (u16, u32) {
        (4095, self.adc.avdd_mv())
    }

    fn delay_ms(&mut self, ms: u32) {
        crate::clock::delay_us(ms as u64 * 1000);
    }
}
