//! Keyboard input from the USB serial port (UART0 RX, GPIO3).
//!
//! An RX interrupt moves incoming bytes into a ring buffer, so nothing is
//! lost while a script runs, and sets [`crate::ABORT`] when Ctrl-C arrives.
//! That stops a running script even inside `while True:` or `sleep_ms()`.
//!
//! Output keeps going through `esp_println` on the same UART: this driver
//! only connects the RX pin and leaves TX alone.

use core::cell::RefCell;
use core::sync::atomic::Ordering;

use critical_section::Mutex;
use esp_hal::{
    handler,
    peripherals::{GPIO3, UART0},
    uart::{Config, RxConfig, Uart, UartInterrupt},
    Blocking,
};

/// Ctrl-C
pub const CTRL_C: u8 = 0x03;

const RING_SIZE: usize = 2048;

struct Ring {
    buf: [u8; RING_SIZE],
    head: usize,
    len: usize,
}

impl Ring {
    fn push(&mut self, byte: u8) {
        if self.len == RING_SIZE {
            return; // full: drop the byte (the REPL reads much faster than 115200 baud)
        }
        self.buf[(self.head + self.len) % RING_SIZE] = byte;
        self.len += 1;
    }

    fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.buf[self.head];
        self.head = (self.head + 1) % RING_SIZE;
        self.len -= 1;
        Some(byte)
    }
}

static UART: Mutex<RefCell<Option<Uart<'static, Blocking>>>> = Mutex::new(RefCell::new(None));
static RING: Mutex<RefCell<Ring>> = Mutex::new(RefCell::new(Ring {
    buf: [0; RING_SIZE],
    head: 0,
    len: 0,
}));

/// Starts receiving on UART0. Call once at boot.
pub fn init(uart0: UART0<'static>, rx: GPIO3<'static>) {
    let config = Config::default().with_rx(RxConfig::default().with_fifo_full_threshold(32));
    let mut uart = Uart::new(uart0, config).unwrap().with_rx(rx);
    uart.set_interrupt_handler(on_uart_rx);
    critical_section::with(|cs| {
        uart.listen(UartInterrupt::RxFifoFull | UartInterrupt::RxTimeout);
        UART.borrow_ref_mut(cs).replace(uart);
    });
}

/// The next received byte, if any.
pub fn read_byte() -> Option<u8> {
    critical_section::with(|cs| RING.borrow_ref_mut(cs).pop())
}

/// Drops queued input up to and including the first `byte`.
pub fn discard_through(byte: u8) {
    critical_section::with(|cs| {
        let mut ring = RING.borrow_ref_mut(cs);
        while let Some(b) = ring.pop() {
            if b == byte {
                break;
            }
        }
    });
}

/// Waits for the next received byte.
pub fn wait_byte() -> u8 {
    loop {
        if let Some(byte) = read_byte() {
            return byte;
        }
    }
}

#[handler]
fn on_uart_rx() {
    critical_section::with(|cs| {
        let mut uart = UART.borrow_ref_mut(cs);
        let Some(uart) = uart.as_mut() else {
            return;
        };
        let mut ring = RING.borrow_ref_mut(cs);
        let mut buf = [0u8; 64];
        while let Ok(n @ 1..) = uart.read_buffered(&mut buf) {
            for &byte in &buf[..n] {
                if byte == CTRL_C {
                    crate::ABORT.store(true, Ordering::Relaxed);
                }
                ring.push(byte);
            }
        }
        uart.clear_interrupts(UartInterrupt::RxFifoFull | UartInterrupt::RxTimeout);
    });
}
