#![no_std]
#![no_main]

//! micropython-rs firmware for ESP32.
//!
//! Build variants (Cargo features):
//! - default      : any ESP32 board. `print()` -> USB/UART serial, `led_on()`/`led_off()` -> GPIO2 LED.
//! - `tft`        : LilyGO TTGO T-Display. `print()` -> ST7789 TFT screen (mirrored to serial).

use esp_backtrace as _;
use esp_hal::{delay::Delay, gpio::Level, main};
use esp_println::println;
use micropython_rs::{Error, HardwareBridge, Interpreter};

esp_bootloader_esp_idf::esp_app_desc!();

/// Arithmetic demo shared by every board, so the same formula and result
/// show up on the serial monitor and on the TFT screen.
/// Expects `a` and `b` to be set earlier in the script.
macro_rules! math_demo {
    () => {
        r#"
result = (a + b) * 3 - 2 ** 4 // 2
print('(a+b)*3-2**4//2 =', result)
"#
    };
}

// ---------------------------------------------------------------------------
// Generic ESP32 board (no display)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "tft"))]
use esp_hal::gpio::{Output as GpioOutput, OutputConfig};
#[cfg(not(feature = "tft"))]
use micropython_esp32::serial_console::EspSerialOut;

/// Exposes the onboard LED + blocking delay to Python scripts.
#[cfg(not(feature = "tft"))]
struct Esp32Board<'d> {
    led: GpioOutput<'d>,
    delay: Delay,
}

#[cfg(not(feature = "tft"))]
impl<'d> HardwareBridge for Esp32Board<'d> {
    fn call(&mut self, name: &str, args: &[i64]) -> Result<Option<i64>, Error> {
        match (name, args) {
            ("led_on", []) => {
                self.led.set_high();
                Ok(None)
            }
            ("led_off", []) => {
                self.led.set_low();
                Ok(None)
            }
            ("delay_ms", [ms]) if *ms >= 0 => {
                self.delay.delay_millis(*ms as u32);
                Ok(None)
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }
}

#[cfg(not(feature = "tft"))]
const SCRIPT: &str = concat!(
    r#"
print('=== micropython-rs ===')
chip = 'ESP32'
a = 21
b = 21
print('Hello from', chip)
print('a =', a, 'b =', b)
"#,
    math_demo!(),
    r#"

print('Blinking onboard LED...')
led_on()
delay_ms(250)
led_off()
delay_ms(250)
led_on()
delay_ms(600)
led_off()
print('LED sequence complete!')
"#
);

#[cfg(not(feature = "tft"))]
#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Most ESP32 DevKit boards wire the onboard LED to GPIO2.
    // If your board uses another pin, swap `peripherals.GPIO2` below.
    let led = GpioOutput::new(peripherals.GPIO2, Level::Low, OutputConfig::default());

    let mut out = EspSerialOut;
    let mut board = Esp32Board {
        led,
        delay: Delay::new(),
    };

    run(Interpreter::with_hardware(&mut out, &mut board))
}

// ---------------------------------------------------------------------------
// LilyGO TTGO T-Display (ST7789 TFT)
// ---------------------------------------------------------------------------

#[cfg(feature = "tft")]
use micropython_esp32::tft_console::{init_ttgo_spi, make_output_pin, TftConsole};

/// Exposes blocking delay to Python scripts (GPIO4 is owned by the TFT backlight).
#[cfg(feature = "tft")]
struct TtgoHardware {
    delay: Delay,
}

#[cfg(feature = "tft")]
impl HardwareBridge for TtgoHardware {
    fn call(&mut self, name: &str, args: &[i64]) -> Result<Option<i64>, Error> {
        match (name, args) {
            ("delay_ms", [ms]) if *ms >= 0 => {
                self.delay.delay_millis(*ms as u32);
                Ok(None)
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }
}

#[cfg(feature = "tft")]
const SCRIPT: &str = concat!(
    r#"
print('=== micropython-rs ===')
board = 'TTGO T-Display'
a = 21
b = 21
print('Board:', board)
print('a =', a, 'b =', b)
"#,
    math_demo!(),
    r#"
delay_ms(500)
print('TFT console active!')
"#
);

#[cfg(feature = "tft")]
#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    let spi = init_ttgo_spi(peripherals.SPI2, peripherals.GPIO18, peripherals.GPIO19);
    let cs = make_output_pin(peripherals.GPIO5, Level::High);
    let dc = make_output_pin(peripherals.GPIO16, Level::Low);
    let rst = make_output_pin(peripherals.GPIO23, Level::High);
    let bl = make_output_pin(peripherals.GPIO4, Level::Low);

    let mut spi_buf = [0u8; 512];
    let mut console = TftConsole::new(spi, cs, dc, rst, bl, &mut spi_buf);
    let mut hw = TtgoHardware {
        delay: Delay::new(),
    };

    run(Interpreter::with_hardware(&mut console, &mut hw))
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

fn run<O: micropython_rs::Output, H: HardwareBridge>(mut vm: Interpreter<'_, O, H>) -> ! {
    match vm.eval(SCRIPT) {
        Ok(()) => println!("=== Execution complete ==="),
        Err(err) => println!("Python Error: {:?}", err),
    }
    loop {}
}
