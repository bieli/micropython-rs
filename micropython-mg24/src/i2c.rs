//! I2C: I2C1 as the bus controller (leader), on any two free pins (the
//! routes of Series 2 chips go to any pin), 7-bit addresses, by polling.
//! mg24-hal's driver has fixed pins (PC4/PC5) and a speed divider for 19 MHz
//! only, so this one follows the reference manual's sequence with the
//! flags (ACK, NACK, RXDATAV, MSTOP) and timeouts in microseconds.
//!
//! I2C1's clock is PCLK (the CPU clock here, below its 50 MHz limit).

use micropython_rs::Error;

use crate::clock;
use crate::gpio::{self, PinId};
use efr32mg24_pac as pac;

/// Longest wait for one bus step (a byte at 10 kHz takes 0.9 ms).
const STEP_TIMEOUT_US: u64 = 25_000;

/// Clears interrupt flags through IF's clear alias (Series 2 registers have
/// SET/CLR/TGL aliases 0x1000/0x2000/0x3000 above them): writing a 1 clears
/// that flag for sure, where a plain write of IF may not.
fn clear_flags(mask: u32) {
    let at = i2c().if_() as *const _ as usize + 0x2000;
    // SAFETY: I2C1's IF_CLR register
    unsafe { (at as *mut u32).write_volatile(mask) };
}

const ALL_FLAGS: u32 = 0x001F_FFFF;

fn i2c() -> &'static pac::i2c1_s::RegisterBlock {
    // SAFETY: I2C1's registers; only this module uses them
    unsafe { &*pac::I2c1S::ptr() }
}

/// The bus, set up by `init`.
pub struct I2c;

impl I2c {
    /// I2C1 on `sda`/`scl` at about `hz` (10 kHz to 400 kHz), from PCLK =
    /// `pclk_hz`. The lines become open drain with the chip's pull-ups
    /// (about 40 kOhm: external pull-ups of a few kOhm are still needed for
    /// 400 kHz).
    pub fn init(sda: PinId, scl: PinId, hz: u32, pclk_hz: u32) -> Result<Self, Error> {
        if sda == scl || !(10_000..=400_000).contains(&hz) {
            return Err(Error::ValueError);
        }
        // SAFETY: CMU's clock enable, the I2C1 routes and I2C1 itself are
        // only touched here
        unsafe {
            let cmu = &*pac::CmuS::ptr();
            cmu.clken0().modify(|_, w| w.i2c1().set_bit());
            gpio::open_drain_pullup(sda);
            gpio::open_drain_pullup(scl);
            let g = &*pac::GpioS::ptr();
            g.i2c1_sdaroute()
                .write(|w| w.port().bits(sda.port).pin().bits(sda.pin));
            g.i2c1_sclroute()
                .write(|w| w.port().bits(scl.port).pin().bits(scl.pin));
            g.i2c1_routeen()
                .write(|w| w.sdapen().set_bit().sclpen().set_bit());
            let i2c = i2c();
            i2c.en().write(|w| w.en().clear_bit());
            // f_SCL = f_PCLK / ((N_low + N_high) * (DIV + 1) + 8): CLHR 0
            // is 4:4 (standard mode), 1 is 6:3 (fast mode)
            let (clhr, n) = if hz <= 100_000 { (0u8, 8) } else { (1u8, 9) };
            let div = ((pclk_hz / hz).saturating_sub(8) / n).saturating_sub(1);
            i2c.ctrl().write(|w| w.clhr().bits(clhr));
            i2c.clkdiv().write(|w| w.div().bits(div.min(511) as u16));
            i2c.ien().write(|w| w.bits(0));
            i2c.en().write(|w| w.en().set_bit());
            // The bus state is unknown after enabling: declare it idle
            i2c.cmd().write(|w| w.abort().set_bit());
        }
        Ok(Self)
    }

    /// Writes `data` to the device at `addr`.
    pub fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Error> {
        let result = self.start(addr, false).and_then(|()| self.send(data));
        self.stop();
        result
    }

    /// Reads `buf.len()` bytes from the device at `addr`.
    pub fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), Error> {
        let result = self.start(addr, true).and_then(|()| self.receive(buf));
        self.stop();
        result
    }

    /// Writes `data`, then reads `buf.len()` bytes after a repeated START
    /// (a register read).
    pub fn write_read(&mut self, addr: u8, data: &[u8], buf: &mut [u8]) -> Result<(), Error> {
        let result = self
            .start(addr, false)
            .and_then(|()| self.send(data))
            .and_then(|()| self.start(addr, true))
            .and_then(|()| self.receive(buf));
        self.stop();
        result
    }

    /// True if a device acknowledges `addr` (an empty write).
    pub fn probe(&mut self, addr: u8) -> bool {
        let found = self.start(addr, false).is_ok();
        self.stop();
        found
    }

    /// START (or a repeated START) and the address; `Err(IoError)` if no
    /// device acknowledges it.
    fn start(&mut self, addr: u8, read: bool) -> Result<(), Error> {
        if addr >= 0x80 {
            return Err(Error::ValueError);
        }
        let i2c = i2c();
        // A previous transfer that went wrong leaves the bus busy: free it
        if i2c.state().read().busy().bit_is_set() && i2c.state().read().state().is_idle() {
            i2c.cmd().write(|w| w.abort().set_bit());
        }
        // A command left from an earlier transfer (a STOP, a NACK) would run
        // right after this START: drop them, the old flags and TXDATA
        i2c.cmd()
            .write(|w| w.clearpc().set_bit().cleartx().set_bit());
        clear_flags(ALL_FLAGS);
        // The address goes into TXDATA first: the controller sends it right
        // after the START condition
        i2c.txdata()
            .write(|w| unsafe { w.txdata().bits(addr << 1 | read as u8) });
        i2c.cmd().write(|w| w.start().set_bit());
        self.wait_ack()
    }

    /// Sends bytes, each acknowledged.
    fn send(&mut self, data: &[u8]) -> Result<(), Error> {
        let i2c = i2c();
        for &byte in data {
            i2c.txdata().write(|w| unsafe { w.txdata().bits(byte) });
            self.wait_ack()?;
        }
        Ok(())
    }

    /// Receives bytes: ACK after each, NACK after the last.
    fn receive(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        let i2c = i2c();
        let last = buf.len().wrapping_sub(1);
        for (i, slot) in buf.iter_mut().enumerate() {
            wait(|| i2c.status().read().rxdatav().bit_is_set())?;
            *slot = i2c.rxdata().read().rxdata().bits();
            if i == last {
                i2c.cmd().write(|w| w.nack().set_bit());
            } else {
                i2c.cmd().write(|w| w.ack().set_bit());
            }
        }
        Ok(())
    }

    /// Waits for the device's ACK (or NACK: `IoError`) of the last byte.
    fn wait_ack(&mut self) -> Result<(), Error> {
        let i2c = i2c();
        wait(|| {
            let f = i2c.if_().read();
            f.ack().bit_is_set()
                || f.nack().bit_is_set()
                || f.arblost().bit_is_set()
                || f.buserr().bit_is_set()
        })?;
        let f = i2c.if_().read();
        clear_flags(f.bits());
        if f.ack().bit_is_set() {
            Ok(())
        } else {
            Err(Error::IoError)
        }
    }

    /// STOP, and waits until it's on the bus.
    fn stop(&mut self) {
        let i2c = i2c();
        i2c.cmd().write(|w| w.stop().set_bit());
        if wait(|| i2c.if_().read().mstop().bit_is_set()).is_err() {
            // The bus is stuck (a line held low): reset the controller's state
            i2c.cmd().write(|w| w.abort().set_bit());
        }
        clear_flags(ALL_FLAGS);
    }
}

/// Waits until `done()`, at most `STEP_TIMEOUT_US`; `Err(IoError)` (a bus
/// without pull-ups, or a device holding a line) after that.
fn wait(done: impl Fn() -> bool) -> Result<(), Error> {
    let until = clock::now_us() + STEP_TIMEOUT_US;
    while !done() {
        if clock::now_us() > until {
            return Err(Error::IoError);
        }
    }
    Ok(())
}
