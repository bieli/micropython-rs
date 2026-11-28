//! The ADC: IADC0, 12 bits, single conversions on any free pin (the analog
//! buses reach every port: ABUS port A, BBUS port B, CDBUS ports C and D).
//!
//! Pins are measured against AVDD (the supply, 0-3.3 V on the XIAO), like
//! the ESP32 boards' 11 dB range. To give millivolts, AVDD itself is measured
//! against the chip's trimmed 1.21 V bandgap (its supply input, divided by
//! 4) when the ADC starts: `adc_mv` = raw * AVDD / 4095.
//!
//! Clocks: FSRCO (20 MHz) as the source, CLK_ADC 10 MHz; the ADC warms up
//! for each conversion (a few microseconds).

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

pub struct Adc {
    /// AVDD in mV, measured at `new`.
    avdd_mv: u32,
}

impl Adc {
    pub fn new() -> Self {
        // SAFETY: the clocks, the analog buses and IADC0's configuration;
        // set once (CTRL, CFG and SCHED can only change while it's disabled)
        unsafe {
            let cmu = &*pac::CmuS::ptr();
            cmu.clken0().modify(|_, w| w.iadc0().set_bit());
            cmu.iadcclkctrl().write(|w| w.bits(2)); // FSRCO, 20 MHz
                                                    // Both even and odd pins of every bus to the ADC
            let g = &*pac::GpioS::ptr();
            g.abusalloc().write(|w| w.aeven0().bits(1).aodd0().bits(1));
            g.bbusalloc().write(|w| w.beven0().bits(1).bodd0().bits(1));
            g.cdbusalloc()
                .write(|w| w.cdeven0().bits(1).cdodd0().bits(1));
            let a = iadc();
            a.en().write(|w| w.en().clear_bit());
            // 1 us time base at 20 MHz; the ADC clock 20 MHz / 2
            a.ctrl().write(|w| w.timebase().bits(19));
            a.sched0().write(|w| w.prescale().bits(1));
            a.sched1().write(|w| w.prescale().bits(1));
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
        let mut adc = Self { avdd_mv: 3300 };
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

    /// The reference: AVDD in mV (`adc_vref`).
    pub fn avdd_mv(&self) -> u32 {
        self.avdd_mv
    }
}
