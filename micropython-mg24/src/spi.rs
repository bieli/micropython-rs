//! SPI: EUSART0 as the main (master) in synchronous mode, 8 bits, MSB first,
//! modes 0-3, on free pins of ports A and B (EUSART0's routes reach only
//! those: the XIAO's SPI pins D8 SCK, D9 MISO, D10 MOSI are PA3-PA5). Chip
//! select is any pin the script drives (`pin_write`), as on the ESP32 boards.
//!
//! The clock: EUSART0's is EM01GRPCCLK (the CPU clock), divided by
//! SDIV + 1 for SCLK.

use micropython_rs::Error;

use crate::gpio::{self, PinId};
use efr32mg24_pac as pac;

fn eusart() -> &'static pac::eusart0_s::RegisterBlock {
    // SAFETY: EUSART0's registers; only this module uses them
    unsafe { &*pac::Eusart0S::ptr() }
}

pub struct Spi;

impl Spi {
    /// EUSART0 on `sck`, `mosi`, `miso` at about `khz` (the fastest SCLK not
    /// above it), SPI mode 0-3, from its clock `clock_hz`.
    pub fn init(
        sck: PinId,
        mosi: PinId,
        miso: PinId,
        khz: u32,
        mode: u8,
        clock_hz: u32,
    ) -> Result<Self, Error> {
        if mode > 3 || khz == 0 || [sck, mosi, miso].iter().any(|p| p.port > 1) {
            return Err(Error::ValueError); // EUSART0 reaches ports A and B only
        }
        let sdiv = clock_hz.div_ceil(khz * 1000).clamp(1, 256) - 1;
        // SAFETY: CMU's clock enables, the EUSART0 routes and EUSART0; its
        // configuration can only be written while it's disabled
        unsafe {
            let cmu = &*pac::CmuS::ptr();
            cmu.clken1().modify(|_, w| w.eusart0().set_bit());
            let idle_high = mode & 2 != 0;
            gpio::set_push_pull(sck, idle_high);
            gpio::set_push_pull(mosi, false);
            if miso != mosi {
                gpio::input_pullup(miso);
            }
            let g = &*pac::GpioS::ptr();
            g.eusart0_sclkroute()
                .write(|w| w.port().bits(sck.port).pin().bits(sck.pin));
            g.eusart0_txroute()
                .write(|w| w.port().bits(mosi.port).pin().bits(mosi.pin));
            g.eusart0_rxroute()
                .write(|w| w.port().bits(miso.port).pin().bits(miso.pin));
            g.eusart0_routeen()
                .write(|w| w.sclkpen().set_bit().txpen().set_bit().rxpen().set_bit());
            let u = eusart();
            u.en().write(|w| w.en().clear_bit());
            while u.en().read().bits() & 0b10 != 0 {} // DISABLING
            u.cfg0().write(|w| w.sync().set_bit().msbf().set_bit());
            // MASTER, CLKPOL (mode 2, 3), CLKPHA (mode 1, 3), SDIV
            u.cfg2().write(|w| {
                w.bits(1 | ((mode as u32 >> 1) & 1) << 1 | (mode as u32 & 1) << 2 | sdiv << 24)
            });
            u.framecfg().write(|w| w.bits(0x1002)); // 8 data bits
            u.en().write(|w| w.en().set_bit());
            while u.syncbusy().read().bits() != 0 {}
            u.cmd().write(|w| w.rxen().set_bit().txen().set_bit());
            while u.syncbusy().read().bits() != 0 {}
        }
        Ok(Self)
    }

    /// Sends each byte of `buf` and replaces it with the byte received meanwhile.
    pub fn transfer(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        let u = eusart();
        // Old bytes in the receive FIFO
        while u.status().read().rxfl().bit_is_set() {
            let _ = u.rxdata().read().bits();
        }
        for byte in buf.iter_mut() {
            while u.status().read().txfl().bit_is_clear() {}
            // SAFETY: any byte is a valid TXDATA value
            u.txdata().write(|w| unsafe { w.bits(*byte as u32) });
            let until = crate::clock::now_us() + 10_000;
            while u.status().read().rxfl().bit_is_clear() {
                if crate::clock::now_us() > until {
                    return Err(Error::IoError);
                }
            }
            *byte = u.rxdata().read().bits() as u8;
        }
        Ok(())
    }
}
