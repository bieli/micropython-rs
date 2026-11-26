//! Keyboard input from the USB serial port: UART0 RX (GPIO3) on the ESP32,
//! whose boards have a USB-to-UART chip; the chip's own USB port
//! (USB-Serial/JTAG) on the ESP32-S3.
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
use esp_hal::{handler, Blocking};
#[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
use esp_hal::{peripherals::USB_DEVICE, usb_serial_jtag::UsbSerialJtag};
#[cfg(feature = "esp32")]
use esp_hal::{
    peripherals::{GPIO3, UART0},
    uart::{Config, RxConfig, Uart, UartInterrupt},
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

#[cfg(feature = "esp32")]
static UART: Mutex<RefCell<Option<Uart<'static, Blocking>>>> = Mutex::new(RefCell::new(None));
#[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
static USB: Mutex<RefCell<Option<UsbSerialJtag<'static, Blocking>>>> =
    Mutex::new(RefCell::new(None));
static RING: Mutex<RefCell<Ring>> = Mutex::new(RefCell::new(Ring {
    buf: [0; RING_SIZE],
    head: 0,
    len: 0,
}));

/// Starts receiving on UART0. Call once at boot.
#[cfg(feature = "esp32")]
pub fn init(uart0: UART0<'static>, rx: GPIO3<'static>) {
    let config = Config::default().with_rx(RxConfig::default().with_fifo_full_threshold(32));
    let mut uart = Uart::new(uart0, config).unwrap().with_rx(rx);
    uart.set_interrupt_handler(on_uart_rx);
    critical_section::with(|cs| {
        uart.listen(UartInterrupt::RxFifoFull | UartInterrupt::RxTimeout);
        UART.borrow_ref_mut(cs).replace(uart);
    });
}

/// Starts receiving on the USB-Serial/JTAG port. Call once at boot. Output
/// keeps going through `esp_println`, which writes to the same port.
#[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
pub fn init(usb: USB_DEVICE<'static>) {
    let mut usb = UsbSerialJtag::new(usb);
    usb.set_interrupt_handler(on_usb_rx);
    critical_section::with(|cs| {
        usb.listen_rx_packet_recv_interrupt();
        USB.borrow_ref_mut(cs).replace(usb);
    });
}

/// Queues received bytes; Ctrl-C also sets the abort flag.
fn received(ring: &mut Ring, bytes: &[u8]) {
    for &byte in bytes {
        if byte == CTRL_C {
            crate::ABORT.store(true, Ordering::Relaxed);
        }
        ring.push(byte);
    }
}

/// When output was last written to the USB-Serial/JTAG port (microseconds
/// since boot, wrapping at 32 bits), or 0 when it has been flushed since.
#[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
static OUTPUT_AT_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
fn now_us() -> u32 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros() as u32
}

/// Notes that output was written (`serial_console`). On the ESP32-S3, the
/// USB-Serial/JTAG port ignores a flush that comes while its previous packet
/// is still being sent: a short write right after another one (the raw
/// REPL's `\x04>`) would wait in its buffer until the next write. So while
/// the board waits for input, `read_byte` flushes once more.
pub fn output_written() {
    #[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
    OUTPUT_AT_US.store(now_us().max(1), Ordering::Relaxed);
}

/// The second flush of `output_written`, 1 ms after the last write (the
/// USB frame time, by when the previous packet has gone).
#[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
fn flush_pending_output() {
    let at = OUTPUT_AT_US.load(Ordering::Relaxed);
    if at == 0 {
        return;
    }
    if now_us().wrapping_sub(at) >= 1000 {
        OUTPUT_AT_US.store(0, Ordering::Relaxed);
        esp_hal::peripherals::USB_DEVICE::regs()
            .ep1_conf()
            .write(|w| w.wr_done().set_bit());
    }
}

/// The next received byte, if any.
pub fn read_byte() -> Option<u8> {
    #[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
    flush_pending_output();
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

#[cfg(feature = "esp32")]
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
            received(&mut ring, &buf[..n]);
        }
        uart.clear_interrupts(UartInterrupt::RxFifoFull | UartInterrupt::RxTimeout);
    });
}

#[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
#[handler]
fn on_usb_rx() {
    critical_section::with(|cs| {
        let mut usb = USB.borrow_ref_mut(cs);
        let Some(usb) = usb.as_mut() else {
            return;
        };
        let mut ring = RING.borrow_ref_mut(cs);
        while let Ok(byte) = usb.read_byte() {
            received(&mut ring, &[byte]);
        }
        usb.reset_rx_packet_recv_interrupt();
    });
}
