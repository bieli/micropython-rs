//! I2C: the bus controller (leader) on two free pins, 7-bit addresses, by
//! polling. The chip routes I2C1 to ports C and D only, I2C0 to any port: a
//! bus on two pins of ports C/D (D0-D7, D18; the default D4/D5) is I2C1,
//! any other I2C0.
//! mg24-hal's driver has fixed pins (PC4/PC5) and a speed divider for 19 MHz
//! only, so this one follows the reference manual's sequence with the
//! flags (ACK, NACK, RXDATAV, MSTOP) and timeouts in microseconds.
//!
//! The clock (`clock_hz` for `init`): PCLK for I2C1, which is the CPU clock
//! here (below its 50 MHz limit); see `I2C0_CLOCK_DIV` for I2C0.

use micropython_rs::Error;

use crate::clock;
use crate::gpio::{self, PinId};
use efr32mg24_pac as pac;

/// Longest wait for one bus step (a byte at 10 kHz takes 0.9 ms).
const STEP_TIMEOUT_US: u64 = 25_000;
/// I2C0 is in the low-power domain, clocked by PCLK / 2: measured on the
/// board, with the Sense's IMU: at a divider for PCLK, a 12-byte transfer
/// took 1550 us longer at 100 kHz than at 400 kHz instead of 750 (the bus
/// ran at half the speed asked for).
const I2C0_CLOCK_DIV: u32 = 2;

/// Clears interrupt flags through IF's clear alias (Series 2 registers have
/// SET/CLR/TGL aliases 0x1000/0x2000/0x3000 above them): writing a 1 clears
/// that flag for sure, where a plain write of IF may not.
fn clear_flags(n: u8, mask: u32) {
    let at = regs(n).if_() as *const _ as usize + 0x2000;
    // SAFETY: I2Cn's IF_CLR register
    unsafe { (at as *mut u32).write_volatile(mask) };
}

const ALL_FLAGS: u32 = 0x001F_FFFF;

/// I2C0's or I2C1's registers (the same layout).
fn regs(n: u8) -> &'static pac::i2c1_s::RegisterBlock {
    // SAFETY: the controller's registers; each belongs to one `I2c` at a time
    unsafe {
        if n == 0 {
            &*(pac::I2c0S::ptr() as *const pac::i2c1_s::RegisterBlock)
        } else {
            &*pac::I2c1S::ptr()
        }
    }
}

/// A bus, set up by `init`: I2C0 or I2C1 (two buses can work at once,
/// one on each).
pub struct I2c {
    n: u8,
    pins: [PinId; 2],
}

impl I2c {
    /// I2C1 on `sda`/`scl` at about `hz` (10 kHz to 400 kHz), from PCLK =
    /// `pclk_hz`. The lines become open drain with the chip's pull-ups
    /// (about 40 kOhm: external pull-ups of a few kOhm are still needed for
    /// 400 kHz).
    pub fn init(sda: PinId, scl: PinId, hz: u32, pclk_hz: u32) -> Result<Self, Error> {
        if sda == scl || !(10_000..=400_000).contains(&hz) {
            return Err(Error::ValueError);
        }
        let n = if sda.port >= 2 && scl.port >= 2 { 1 } else { 0 };
        let clock_hz = if n == 0 {
            pclk_hz / I2C0_CLOCK_DIV
        } else {
            pclk_hz
        };
        // SAFETY: CMU's clock enables, the I2C routes and the I2C
        // controllers are only touched here
        unsafe {
            let cmu = &*pac::CmuS::ptr();
            cmu.clken0()
                .modify(|_, w| w.i2c0().set_bit().i2c1().set_bit());
            gpio::open_drain_pullup(sda);
            gpio::open_drain_pullup(scl);
            let g = &*pac::GpioS::ptr();
            if n == 0 {
                g.i2c0_sdaroute()
                    .write(|w| w.port().bits(sda.port).pin().bits(sda.pin));
                g.i2c0_sclroute()
                    .write(|w| w.port().bits(scl.port).pin().bits(scl.pin));
                g.i2c0_routeen()
                    .write(|w| w.sdapen().set_bit().sclpen().set_bit());
            } else {
                g.i2c1_sdaroute()
                    .write(|w| w.port().bits(sda.port).pin().bits(sda.pin));
                g.i2c1_sclroute()
                    .write(|w| w.port().bits(scl.port).pin().bits(scl.pin));
                g.i2c1_routeen()
                    .write(|w| w.sdapen().set_bit().sclpen().set_bit());
            }
            let i2c = regs(n);
            i2c.en().write(|w| w.en().clear_bit());
            // f_SCL = f_PCLK / ((N_low + N_high) * (DIV + 1) + 8): CLHR 0
            // is 4:4 (standard mode), 1 is 6:3 (fast mode)
            let (clhr, n) = if hz <= 100_000 { (0u8, 8) } else { (1u8, 9) };
            let div = ((clock_hz / hz).saturating_sub(8) / n).saturating_sub(1);
            i2c.ctrl().write(|w| w.clhr().bits(clhr));
            i2c.clkdiv().write(|w| w.div().bits(div.min(511) as u16));
            i2c.ien().write(|w| w.bits(0));
            i2c.en().write(|w| w.en().set_bit());
            // The bus state is unknown after enabling: declare it idle
            i2c.cmd().write(|w| w.abort().set_bit());
        }
        Ok(Self {
            n,
            pins: [sda, scl],
        })
    }

    /// SDA and SCL.
    pub fn pins(&self) -> [PinId; 2] {
        self.pins
    }

    /// Takes the controller off its pins (the routes) and disconnects them.
    pub fn release(&self) {
        // SAFETY: the routes of this bus's controller
        unsafe {
            let g = &*pac::GpioS::ptr();
            if self.n == 0 {
                g.i2c0_routeen().write(|w| w.bits(0));
            } else {
                g.i2c1_routeen().write(|w| w.bits(0));
            }
        }
        regs(self.n).en().write(|w| w.en().clear_bit());
        self.pins.into_iter().for_each(gpio::release);
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
        let i2c = regs(self.n);
        // A previous transfer that went wrong leaves the bus busy: free it
        if i2c.state().read().busy().bit_is_set() && i2c.state().read().state().is_idle() {
            i2c.cmd().write(|w| w.abort().set_bit());
        }
        // A command left from an earlier transfer (a STOP, a NACK) would run
        // right after this START: drop them, the old flags and TXDATA
        i2c.cmd()
            .write(|w| w.clearpc().set_bit().cleartx().set_bit());
        clear_flags(self.n, ALL_FLAGS);
        // START first, then the address: in the middle of a transfer (a
        // repeated START, before a register is read) the controller is
        // still a transmitter, and a byte in TXDATA before the START would
        // go out as data
        i2c.cmd().write(|w| w.start().set_bit());
        i2c.txdata()
            .write(|w| unsafe { w.txdata().bits(addr << 1 | read as u8) });
        self.wait_ack()
    }

    /// Sends bytes, each acknowledged.
    fn send(&mut self, data: &[u8]) -> Result<(), Error> {
        let i2c = regs(self.n);
        for &byte in data {
            i2c.txdata().write(|w| unsafe { w.txdata().bits(byte) });
            self.wait_ack()?;
        }
        Ok(())
    }

    /// Receives bytes: ACK after each, NACK after the last.
    fn receive(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        let i2c = regs(self.n);
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
        let i2c = regs(self.n);
        wait(|| {
            let f = i2c.if_().read();
            f.ack().bit_is_set()
                || f.nack().bit_is_set()
                || f.arblost().bit_is_set()
                || f.buserr().bit_is_set()
        })?;
        let f = i2c.if_().read();
        clear_flags(self.n, f.bits());
        if f.ack().bit_is_set() {
            Ok(())
        } else {
            Err(Error::IoError)
        }
    }

    /// STOP, and waits until it's on the bus.
    fn stop(&mut self) {
        let i2c = regs(self.n);
        i2c.cmd().write(|w| w.stop().set_bit());
        if wait(|| i2c.if_().read().mstop().bit_is_set()).is_err() {
            // The bus is stuck (a line held low): reset the controller's state
            i2c.cmd().write(|w| w.abort().set_bit());
        }
        clear_flags(self.n, ALL_FLAGS);
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
