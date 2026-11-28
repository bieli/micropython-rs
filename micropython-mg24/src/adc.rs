//! The ADC: IADC0, 12 bits, single conversions on any free pin (the analog
//! buses reach every port: ABUS port A, BBUS port B, CDBUS ports C and D).
//!
//! Pins are measured against AVDD (the supply, 0-3.3 V on the XIAO), like
//! the ESP32 boards' 11 dB range. To give millivolts, AVDD itself is measured
//! against the chip's trimmed 1.21 V bandgap (its supply input, divided by
//! 4) when the ADC starts: `adc_mv` = raw * AVDD / 4095.
//!
//! Clocks: EM01GRPACLK as the source (CLK_SRC_ADC: the 39 MHz crystal, see
//! `sysclock`), CLK_ADC a quarter of it (at most 10 MHz); the ADC warms up
//! for each conversion (a few microseconds).
//!
//! Timed sampling (`sample`, for the `mic` module): the IADC's own timer
//! triggers each conversion, every `period` cycles of CLK_SRC_ADC, so the
//! samples are evenly spaced (no software jitter) at the crystal's
//! accuracy; the CPU only empties the 4-entry FIFO.

use micropython_rs::Error;

use crate::gpio::PinId;
use efr32mg24_pac as pac;

const FULL_SCALE: u32 = 4095;
/// The bandgap reference, in mV.
const VBGR_MV: u32 = 1210;
/// SINGLE.PORTPOS for the chip's supplies; PINPOS 0 is AVDD (divided by 4).
const PORT_SUPPLY: u8 = 1;
const PORT_A: u8 = 8;

fn iadc() -> &'static pac::iadc0_s::RegisterBlock {
    // SAFETY: IADC0's registers; only this module uses them
    unsafe { &*pac::Iadc0S::ptr() }
}

/// Timed sampling's rates: the timer's 16-bit period and the conversion
/// time (warm-up included) bound them.
const RATES: core::ops::RangeInclusive<u32> = 1000..=48_000;

pub struct Adc {
    /// AVDD in mV, measured at `new`.
    avdd_mv: u32,
    /// CLK_SRC_ADC, Hz.
    src_hz: u32,
}

impl Adc {
    /// IADC0 from EM01GRPACLK at `src_hz` (at most 40 MHz).
    pub fn new(src_hz: u32) -> Self {
        // SAFETY: the clocks, the analog buses and IADC0's configuration;
        // set once (CTRL, CFG and SCHED can only change while it's disabled)
        unsafe {
            let cmu = &*pac::CmuS::ptr();
            cmu.clken0().modify(|_, w| w.iadc0().set_bit());
            cmu.iadcclkctrl().write(|w| w.bits(1)); // EM01GRPACLK
                                                    // Both even and odd pins of every bus to the ADC
            let g = &*pac::GpioS::ptr();
            g.abusalloc().write(|w| w.aeven0().bits(1).aodd0().bits(1));
            g.bbusalloc().write(|w| w.beven0().bits(1).bodd0().bits(1));
            g.cdbusalloc()
                .write(|w| w.cdeven0().bits(1).cdodd0().bits(1));
            let a = iadc();
            a.en().write(|w| w.en().clear_bit());
            // The time base: CLK_SRC_ADC cycles per microsecond, minus 1;
            // CLK_ADC = CLK_SRC_ADC / (PRESCALE + 1), at most 10 MHz
            let mhz = src_hz.div_ceil(1_000_000);
            let prescale = src_hz.div_ceil(10_000_000) - 1;
            a.ctrl().write(|w| w.timebase().bits((mhz - 1) as u8));
            a.sched0().write(|w| w.prescale().bits(prescale as u16));
            a.sched1().write(|w| w.prescale().bits(prescale as u16));
            // CFG0: AVDD as the reference, gain 1 (pins, 0-3.3 V);
            // CFG1: the bandgap, gain 1 (AVDD / 4, about 0.8 V)
            a.cfg0()
                .write(|w| w.osrhs().bits(0).analoggain().bits(2).refsel().bits(3));
            a.cfg1()
                .write(|w| w.osrhs().bits(0).analoggain().bits(2).refsel().bits(0));
            a.singlefifocfg()
                .write(|w| w.alignment().bits(0).dvl().bits(0));
            a.en().write(|w| w.en().set_bit());
        }
        let mut adc = Self {
            avdd_mv: 3300,
            src_hz,
        };
        let raw = adc.convert(PORT_SUPPLY, 0, 1).unwrap_or(0);
        let avdd = raw * VBGR_MV * 4 / FULL_SCALE;
        // Keep the nominal 3.3 V if the measurement is far off (a broken reading)
        if (1700..=3700).contains(&avdd) {
            adc.avdd_mv = avdd;
        }
        adc
    }

    /// One conversion of (port, pin) with configuration `cfg`.
    fn convert(&mut self, port: u8, pin: u8, cfg: u8) -> Result<u32, Error> {
        let a = iadc();
        // Empty the FIFO of an older result
        while a.status().read().singlefifodv().bit_is_set() {
            let _ = a.singlefifodata().read().bits();
        }
        // SAFETY: the input selection and its configuration
        a.single().write(|w| unsafe {
            w.portpos()
                .bits(port)
                .pinpos()
                .bits(pin)
                .portneg()
                .bits(0)
                .cfg()
                .bit(cfg == 1)
        });
        while a.status().read().singlewritepending().bit_is_set() {}
        a.cmd().write(|w| w.singlestart().set_bit());
        let until = crate::clock::now_us() + 10_000;
        while a.status().read().singlefifodv().bit_is_clear() {
            if crate::clock::now_us() > until {
                return Err(Error::IoError);
            }
        }
        Ok(a.singlefifodata().read().bits() & 0xFFF)
    }

    /// `adc_read(pin)`: the raw 12-bit value (0-4095 for 0 V to AVDD).
    pub fn read(&mut self, id: PinId) -> Result<u32, Error> {
        self.convert(PORT_A + id.port, id.pin, 0)
    }

    /// `adc_mv(pin, samples)`: millivolts, averaged.
    pub fn read_mv(&mut self, id: PinId, samples: u32) -> Result<u32, Error> {
        let mut sum = 0u32;
        for _ in 0..samples.max(1) {
            sum += self.read(id)?;
        }
        let raw = sum / samples.max(1);
        Ok((raw * self.avdd_mv + FULL_SCALE / 2) / FULL_SCALE)
    }

    /// The timer's period for about `rate_hz`, in CLK_SRC_ADC cycles (TIMER
    /// gets one less: measured over 5 s against the microsecond counter,
    /// both on the crystal, a TIMER of 884 gave 44 068.7 Hz, 39 MHz / 885).
    fn period(&self, rate_hz: u32) -> Result<u32, Error> {
        if !RATES.contains(&rate_hz) {
            return Err(Error::ValueError);
        }
        let p = (self.src_hz + rate_hz / 2) / rate_hz;
        if p > 0x1_0000 {
            return Err(Error::ValueError);
        }
        Ok(p)
    }

    /// The rate `sample` really uses for `rate_hz` (a whole number of cycles).
    pub fn actual_rate(&self, rate_hz: u32) -> Result<f64, Error> {
        Ok(self.src_hz as f64 / self.period(rate_hz)? as f64)
    }

    /// Changes registers that need IADC0 disabled (TIMER, TRIGGER).
    fn reconfigure(&mut self, change: impl FnOnce(&pac::iadc0_s::RegisterBlock)) {
        let a = iadc();
        a.en().write(|w| w.en().clear_bit());
        while a.en().read().bits() & 0b10 != 0 {} // DISABLING
        change(a);
        a.en().write(|w| w.en().set_bit());
    }

    /// `n` conversions of `id`, triggered by the IADC's timer at about
    /// `rate_hz` (see `actual_rate`), each given to `each` as it arrives.
    /// `Err(IoError)` if one doesn't come, or if the FIFO overflowed (the
    /// CPU was held up longer than four samples: one was lost).
    pub fn sample(
        &mut self,
        id: PinId,
        rate_hz: u32,
        n: usize,
        each: &mut dyn FnMut(u16),
    ) -> Result<(), Error> {
        let period = self.period(rate_hz)?;
        // SAFETY: the timer's period and the trigger (the single queue,
        // triggered by the timer, one conversion each), written disabled
        self.reconfigure(|a| unsafe {
            a.timer().write(|w| w.bits(period - 1));
            a.trigger().write(|w| w.bits(1 << 8));
        });
        let a = iadc();
        // SAFETY: the input selection, the commands and IF's clear alias
        unsafe {
            a.single().write(|w| {
                w.portpos()
                    .bits(PORT_A + id.port)
                    .pinpos()
                    .bits(id.pin)
                    .portneg()
                    .bits(0)
                    .cfg()
                    .clear_bit()
            });
            while a.status().read().singlewritepending().bit_is_set() {}
            a.cmd().write(|w| w.bits(1 << 24)); // flush the FIFO
            while a.status().read().bits() & 1 << 14 != 0 {}
            ((a.if_() as *const _ as usize + 0x2000) as *mut u32).write_volatile(1 << 16);
            a.cmd().write(|w| w.bits(1)); // start the single queue
            a.cmd().write(|w| w.bits(1 << 16)); // and the timer
        }
        let mut result = Ok(());
        'samples: for _ in 0..n {
            let until = crate::clock::now_us() + 10_000;
            while a.status().read().singlefifodv().bit_is_clear() {
                if crate::clock::now_us() > until {
                    result = Err(Error::IoError);
                    break 'samples;
                }
            }
            if a.if_().read().bits() & 1 << 16 != 0 {
                result = Err(Error::IoError); // the FIFO overflowed
                break;
            }
            each((a.singlefifodata().read().bits() & 0xFFF) as u16);
        }
        // SAFETY: stop the timer and the queue; then single conversions on
        // command again (`convert`)
        unsafe {
            a.cmd().write(|w| w.bits(1 << 17));
            a.cmd().write(|w| w.bits(1 << 1));
        }
        self.reconfigure(|a| unsafe {
            a.trigger().write(|w| w.bits(0));
        });
        result
    }

    /// The reference: AVDD in mV (`adc_vref`).
    pub fn avdd_mv(&self) -> u32 {
        self.avdd_mv
    }
}
