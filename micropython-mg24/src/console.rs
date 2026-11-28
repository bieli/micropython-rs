//! The console: USART0 on PA8 (TX) and PA9 (RX), which the board's SAMD11
//! bridges to its USB port (`/dev/ttyACM0` on Linux), 115200 8N1.
//!
//! Received bytes go into a ring buffer from the receive interrupt, so none
//! is lost while a script runs or the board is printing, and Ctrl-C sets
//! `micropython_port::ABORT` (the interpreter stops the script). The ring has
//! one writer (the interrupt) and one reader (the REPL), so its two indices
//! are atomics and no critical section is needed. Sending waits for room in
//! the transmitter (at 115200 baud a byte takes 87 us).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use efr32mg24_pac as pac;

pub const BAUD: u32 = 115_200;
const CTRL_C: u8 = 0x03;
/// The PAC's interrupt number of USART0's receiver.
const USART0_RX_IRQ: u8 = 9;

const RING_SIZE: usize = 2048;

/// Received bytes: `HEAD` is where the interrupt writes the next one,
/// `TAIL` where the REPL reads; empty when they're equal (so it holds
/// `RING_SIZE - 1` bytes).
struct Ring(UnsafeCell<[u8; RING_SIZE]>);

// SAFETY: the interrupt only writes the slot at HEAD before publishing it,
// the reader only reads slots between TAIL and HEAD (see push/pop)
unsafe impl Sync for Ring {}

static RING: Ring = Ring(UnsafeCell::new([0; RING_SIZE]));
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);
/// Bytes dropped because the ring was full (the REPL reads much faster).
static DROPPED: AtomicUsize = AtomicUsize::new(0);

fn push(byte: u8) {
    let head = HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % RING_SIZE;
    if next == TAIL.load(Ordering::Acquire) {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // SAFETY: only the interrupt writes, and only the free slot at HEAD
    unsafe { (*RING.0.get())[head] = byte };
    HEAD.store(next, Ordering::Release);
}

/// The next received byte, if any.
pub fn read_byte() -> Option<u8> {
    let tail = TAIL.load(Ordering::Relaxed);
    if tail == HEAD.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: the slot at TAIL was published by the interrupt (HEAD moved past it)
    let byte = unsafe { (*RING.0.get())[tail] };
    TAIL.store((tail + 1) % RING_SIZE, Ordering::Release);
    Some(byte)
}

/// Drops queued input up to and including the first `byte`.
pub fn discard_through(byte: u8) {
    while let Some(b) = read_byte() {
        if b == byte {
            break;
        }
    }
}

/// Bytes lost to a full ring since boot (for `lscpu`).
pub fn dropped() -> usize {
    DROPPED.load(Ordering::Relaxed)
}

fn usart() -> &'static pac::usart0_s::RegisterBlock {
    // SAFETY: USART0's registers; after `init` only this module uses them
    unsafe { &*pac::Usart0S::ptr() }
}

/// Sets up USART0 (8N1, 16x oversampling) from PCLK = `pclk_hz` and enables
/// its receive interrupt. The pins must already be a push-pull output (TX)
/// and an input (RX).
pub fn init(pclk_hz: u32) {
    // SAFETY: called once at boot, before the interrupt is enabled; CMU and
    // the USART0 routes are only touched here
    unsafe {
        let cmu = &*pac::CmuS::ptr();
        cmu.clken0().modify(|_, w| w.usart0().set_bit());
        let gpio = &*pac::GpioS::ptr();
        gpio.usart0_txroute()
            .write(|w| w.port().bits(0).pin().bits(8)); // PA8
        gpio.usart0_rxroute()
            .write(|w| w.port().bits(0).pin().bits(9)); // PA9
        gpio.usart0_routeen()
            .write(|w| w.txpen().set_bit().rxpen().set_bit());
        let usart = usart();
        usart.en().write(|w| w.en().set_bit());
        // CLKDIV.DIV has 5 fractional bits: 32 * (pclk / (16 * baud) - 1),
        // rounded (19 MHz, 115200: 298, 115151 baud)
        let div = (2 * pclk_hz + BAUD / 2) / BAUD - 32;
        usart.clkdiv().write(|w| w.div().bits(div));
        usart
            .cmd()
            .write(|w| w.cleartx().set_bit().clearrx().set_bit());
        usart.cmd().write(|w| w.txen().set_bit().rxen().set_bit());
        usart.ien().write(|w| w.rxdatav().set_bit());
        mg24_hal::interrupt::enable_irq(USART0_RX_IRQ);
    }
}

/// USART0's receive interrupt: moves every waiting byte into the ring.
#[unsafe(no_mangle)]
pub extern "C" fn USART0_RX() {
    let usart = usart();
    // The flag first: a byte arriving while the loop runs raises it again
    usart.if_().modify(|_, w| w.rxdatav().clear_bit());
    while usart.status().read().rxdatav().bit_is_set() {
        let byte = usart.rxdata().read().rxdata().bits();
        if byte == CTRL_C {
            micropython_port::ABORT.store(true, Ordering::Relaxed);
        }
        push(byte);
    }
}

fn write_byte(byte: u8) {
    let usart = usart();
    while usart.status().read().txbl().bit_is_clear() {}
    // SAFETY: any byte is a valid TXDATA value
    usart.txdata().write(|w| unsafe { w.txdata().bits(byte) });
}

/// Writes `s` as it is (the raw REPL's framing has its own `\r\n`).
pub fn write_raw(s: &str) {
    for byte in s.bytes() {
        write_byte(byte);
    }
}

/// Writes `s` with `\r\n` line endings, which terminals and MicroPython
/// tools expect (script output, the REPL's prompts).
pub fn write_crlf(s: &str) {
    for byte in s.bytes() {
        if byte == b'\n' {
            write_byte(b'\r');
        }
        write_byte(byte);
    }
}

/// Waits until the last byte has left the transmitter (before a reset).
pub fn flush() {
    while usart().status().read().txc().bit_is_clear() {}
}
