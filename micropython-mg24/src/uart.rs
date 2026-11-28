//! UART: EUSART1 (it reaches every port), 8N1, on any two free pins.
//! Received bytes go into a ring from the receive interrupt (in RAM, like
//! the console's: nothing is lost while a script runs or a file is saved);
//! sending waits for room in the transmit FIFO.
//!
//! The baud rate: EUSART1's clock is EM01GRPCCLK (HFRCODPLL, the CPU clock),
//! 16x oversampling. CLKDIV's DIV field (bits 3-22) is the divider minus 1
//! with 5 fractional bits: 32 * (f / (16 * baud) - 1) (as the register's
//! value it has 8, with its 3 low bits 0: measured, writing 256 * (...) into
//! the field made the line 8 times slower).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use micropython_rs::Error;

use crate::gpio::{self, PinId};
use efr32mg24_pac as pac;

const EUSART1_RX_IRQ: u8 = 13;
const RING_SIZE: usize = 1024;

struct Ring(UnsafeCell<[u8; RING_SIZE]>);
// SAFETY: one writer (the interrupt, the slot at HEAD) and one reader (the
// slots between TAIL and HEAD), as the console's ring
unsafe impl Sync for Ring {}
static RING: Ring = Ring(UnsafeCell::new([0; RING_SIZE]));
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
fn eusart() -> &'static pac::eusart1_s::RegisterBlock {
    // SAFETY: EUSART1's registers; only this module uses them
    unsafe { &*pac::Eusart1S::ptr() }
}

/// EUSART1's receive interrupt: the FIFO into the ring. From RAM.
#[unsafe(no_mangle)]
#[link_section = ".ram_text.eusart1_rx"]
pub extern "C" fn EUSART1_RX() {
    let u = eusart();
    // SAFETY: EUSART1_IF_CLR (the CLR alias, 0x2000 above IF)
    unsafe { ((u.if_() as *const _ as usize + 0x2000) as *mut u32).write_volatile(0b1100) };
    while u.status().read().rxfl().bit_is_set() {
        let byte = u.rxdata().read().bits() as u8;
        let head = HEAD.load(Ordering::Relaxed);
        let next = (head + 1) % RING_SIZE;
        if next != TAIL.load(Ordering::Acquire) {
            // SAFETY: only the interrupt writes, the free slot at HEAD
            unsafe { (*RING.0.get())[head % RING_SIZE] = byte };
            HEAD.store(next, Ordering::Release);
        }
    }
}

pub struct Uart;

impl Uart {
    /// EUSART1 on `tx`/`rx` at `baud` (300 to 2 000 000), from its clock `hz`.
    pub fn init(tx: PinId, rx: PinId, baud: u32, hz: u32) -> Result<Self, Error> {
        if !(300..=2_000_000).contains(&baud) || baud * 16 > hz {
            return Err(Error::ValueError);
        }
        let div = (32 * hz as u64 / (16 * baud as u64)).saturating_sub(32);
        if div >= 1 << 20 {
            return Err(Error::ValueError);
        }
        // SAFETY: CMU's clock enable, the EUSART1 routes and EUSART1; its
        // configuration can only be written while it's disabled
        unsafe {
            (&*pac::CmuS::ptr())
                .clken1()
                .modify(|_, w| w.eusart1().set_bit());
            gpio::set_push_pull(tx, true); // idle high
            if rx != tx {
                gpio::input_pullup(rx); // on the TX pin itself (a loopback) it stays the output
            }
            let g = &*pac::GpioS::ptr();
            g.eusart1_txroute()
                .write(|w| w.port().bits(tx.port).pin().bits(tx.pin));
            g.eusart1_rxroute()
                .write(|w| w.port().bits(rx.port).pin().bits(rx.pin));
            g.eusart1_routeen()
                .write(|w| w.txpen().set_bit().rxpen().set_bit());
            let u = eusart();
            u.en().write(|w| w.en().clear_bit());
            while u.en().read().bits() & 0b10 != 0 {} // DISABLING
            u.cfg0().write(|w| w.bits(0)); // asynchronous, 16x oversampling, LSB first
            u.framecfg().write(|w| w.bits(0x1002)); // 8 data bits, no parity, 1 stop bit
            u.clkdiv().write(|w| w.div().bits(div as u32));
            u.ien().write(|w| w.rxfl().set_bit());
            u.en().write(|w| w.en().set_bit());
            while u.syncbusy().read().bits() != 0 {}
            u.cmd().write(|w| w.rxen().set_bit().txen().set_bit());
            while u.syncbusy().read().bits() != 0 {}
            HEAD.store(0, Ordering::Relaxed);
            TAIL.store(0, Ordering::Relaxed);
            mg24_hal::interrupt::enable_irq(EUSART1_RX_IRQ);
        }
        Ok(Self)
    }

    pub fn write(&mut self, data: &[u8]) {
        let u = eusart();
        for &byte in data {
            while u.status().read().txfl().bit_is_clear() {}
            // SAFETY: any byte is a valid TXDATA value
            u.txdata().write(|w| unsafe { w.bits(byte as u32) });
        }
    }

    /// Bytes waiting in the ring.
    pub fn any(&self) -> usize {
        (HEAD.load(Ordering::Acquire) + RING_SIZE - TAIL.load(Ordering::Relaxed)) % RING_SIZE
    }

    /// Moves waiting bytes into `out`; how many.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let mut n = 0;
        while n < out.len() {
            let tail = TAIL.load(Ordering::Relaxed);
            if tail == HEAD.load(Ordering::Acquire) {
                break;
            }
            // SAFETY: the slot at TAIL was published by the interrupt
            out[n] = unsafe { (*RING.0.get())[tail] };
            TAIL.store((tail + 1) % RING_SIZE, Ordering::Release);
            n += 1;
        }
        n
    }
}
