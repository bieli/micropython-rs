#![no_std]
#![no_main]

//! micropython-rs firmware for ESP32.
//!
//! Build variants (Cargo features):
//! - default : any ESP32 board. `print()` -> USB/UART serial, LED on GPIO2.
//! - `tft`   : LilyGO TTGO T-Display. `print()` -> ST7789 TFT screen (mirrored to serial),
//!   plus drawing functions (`cls`, `color`, `text`, `rect`, `backlight`).
//!
//! Both builds get the shared hardware functions from `board.rs`
//! (time, GPIO, ADC, PWM, I2C, watchdog). Run `help()` in a script to list them.
//!
//! At boot the board runs `main.py` from flash (or the embedded script),
//! then starts the REPL on the USB serial port (`repl.rs`).

use esp_backtrace as _;
use esp_hal::main;
use esp_println::println;
use micropython_esp32::board::{Board, BoardPeripherals, TIME_MODULE};
use micropython_esp32::repl::{self, BootScript};
use micropython_esp32::script_store::ScriptStore;
use micropython_esp32::serial_input;
use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Module};

esp_bootloader_esp_idf::esp_app_desc!();

/// The Python script to run, chosen at build time by `build.rs`:
/// `make flash SCRIPT=scripts/blink.py`, or the board's demo from `scripts/`.
const SCRIPT: &str = include_str!(env!("MPY_SCRIPT_PATH"));

/// Takes the peripherals `Board` needs out of `esp_hal::init`'s result.
macro_rules! board_peripherals {
    ($p:ident) => {
        BoardPeripherals {
            adc1: $p.ADC1,
            gpio34: $p.GPIO34,
            gpio36: $p.GPIO36,
            gpio39: $p.GPIO39,
            ledc: $p.LEDC,
            i2c0: $p.I2C0,
            sda: $p.GPIO21,
            scl: $p.GPIO22,
            lpwr: $p.LPWR,
        }
    };
}

// ---------------------------------------------------------------------------
// Generic ESP32 board (no display)
// ---------------------------------------------------------------------------

/// Pins scripts may use: everything except flash (6-11), UART0 (1, 3),
/// I2C (21, 22) and the ADC inputs (34, 36, 39).
#[cfg(not(feature = "tft"))]
const GENERIC_PINS: &[u8] = &[
    0, 2, 4, 5, 12, 13, 14, 15, 16, 17, 18, 19, 23, 25, 26, 27, 32, 33, 35,
];

/// Most ESP32 DevKit boards wire the onboard LED to GPIO2.
#[cfg(not(feature = "tft"))]
const LED_PIN: i64 = 2;

#[cfg(not(feature = "tft"))]
const GENERIC_FNS: &[HwFn] = micropython_esp32::board_functions!(
    HwFn::new("led_on", "", "Onboard LED on (GPIO2)"),
    HwFn::new("led_off", "", "Onboard LED off"),
);

#[cfg(not(feature = "tft"))]
const GENERIC_CONSTS: &[(&str, i64)] =
    micropython_esp32::board_constants!(("LED", LED_PIN), ("BOOT_BUTTON", 0));

#[cfg(not(feature = "tft"))]
struct GenericHardware<'d> {
    board: Board<'d>,
}

#[cfg(not(feature = "tft"))]
impl HardwareBridge for GenericHardware<'_> {
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        match name {
            "led_on" => self
                .board
                .call("pin_write", &[Arg::Int(LED_PIN), Arg::Int(1)]),
            "led_off" => self
                .board
                .call("pin_write", &[Arg::Int(LED_PIN), Arg::Int(0)]),
            _ => self.board.call(name, args),
        }
    }

    fn functions(&self) -> &[HwFn] {
        GENERIC_FNS
    }

    fn constants(&self) -> &[(&'static str, i64)] {
        GENERIC_CONSTS
    }

    fn modules(&self) -> &[Module] {
        &[TIME_MODULE]
    }

    fn call_module(
        &mut self,
        module: &str,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        self.board.call_module(module, name, args)
    }
}

#[cfg(not(feature = "tft"))]
#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    // Set up serial input first: it resets UART0, which drops unsent output
    serial_input::init(peripherals.UART0, peripherals.GPIO3);
    print_reset_reason();
    let mut store = ScriptStore::new(peripherals.FLASH);

    let board = Board::new(board_peripherals!(peripherals), GENERIC_PINS);
    let mut out = micropython_esp32::serial_console::EspSerialOut;
    let mut hw = GenericHardware { board };

    repl::run(&mut out, &mut hw, &BOOT_SCRIPT, &mut store)
}

// ---------------------------------------------------------------------------
// LilyGO TTGO T-Display (ST7789 TFT)
// ---------------------------------------------------------------------------

#[cfg(feature = "tft")]
use core::cell::RefCell;
#[cfg(feature = "tft")]
use esp_hal::gpio::Level;
#[cfg(feature = "tft")]
use micropython_esp32::display_module::{self, DISPLAY_MODULE};
#[cfg(feature = "tft")]
use micropython_esp32::tft_console::{init_ttgo_spi, make_output_pin, SharedConsole, TftConsole};

/// Pins scripts may use: the generic set minus the display pins
/// (4 backlight, 5 CS, 16 DC, 18 SCLK, 19 MOSI, 23 RST).
#[cfg(feature = "tft")]
const TTGO_PINS: &[u8] = &[0, 2, 12, 13, 14, 15, 17, 25, 26, 27, 32, 33, 35];

#[cfg(feature = "tft")]
const TTGO_FNS: &[HwFn] = micropython_esp32::board_functions!(
    HwFn::new("cls", "", "Clear the screen"),
    HwFn::new("color", "r, g, b", "Text/fill color, 0-255 each"),
    HwFn::new("text", "x, y, s", "Draw text at pixel x, y"),
    HwFn::new(
        "rect",
        "x, y, w, h",
        "Fill a rectangle in the current color"
    ),
    HwFn::new("backlight", "on", "Display backlight on/off"),
);

/// The TTGO's two buttons (pressed = 0) and its battery voltage divider,
/// which is only connected while `ADC_EN` is high.
#[cfg(feature = "tft")]
const TTGO_CONSTS: &[(&str, i64)] = micropython_esp32::board_constants!(
    ("BUTTON_L", 0),
    ("BUTTON_R", 35),
    ("BAT_ADC", 34),
    ("ADC_EN", 14),
    ("WIDTH", 240),
    ("HEIGHT", 135),
);

#[cfg(feature = "tft")]
struct TtgoHardware<'a, 'd> {
    board: Board<'d>,
    console: &'a RefCell<TftConsole<'d>>,
}

#[cfg(feature = "tft")]
impl HardwareBridge for TtgoHardware<'_, '_> {
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        let int = |i: usize| args[i].int();
        let coord = |i: usize| -> Result<i32, Error> {
            i32::try_from(args[i].int()?).map_err(|_| Error::ValueError)
        };
        let size = |i: usize| -> Result<u32, Error> {
            u32::try_from(args[i].int()?).map_err(|_| Error::ValueError)
        };
        let component = |i: usize| -> Result<u8, Error> {
            u8::try_from(args[i].int()?).map_err(|_| Error::ValueError)
        };

        let mut console = self.console.borrow_mut();
        match name {
            "cls" => console.clear(),
            "color" => console.set_color(component(0)?, component(1)?, component(2)?),
            "text" => console.draw_text(coord(0)?, coord(1)?, args[2].str()?),
            "rect" => console.fill_rect(coord(0)?, coord(1)?, size(2)?, size(3)?, true),
            "backlight" => console.set_backlight(int(0)? != 0),
            _ => {
                drop(console);
                return self.board.call(name, args);
            }
        }
        Ok(None)
    }

    fn functions(&self) -> &[HwFn] {
        TTGO_FNS
    }

    fn constants(&self) -> &[(&'static str, i64)] {
        TTGO_CONSTS
    }

    fn modules(&self) -> &[Module] {
        &[TIME_MODULE, DISPLAY_MODULE]
    }

    fn call_module(
        &mut self,
        module: &str,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        match module {
            "display" => display_module::call(&mut self.console.borrow_mut(), name, args),
            _ => self.board.call_module(module, name, args),
        }
    }
}

#[cfg(feature = "tft")]
#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    // Set up serial input first: it resets UART0, which drops unsent output
    serial_input::init(peripherals.UART0, peripherals.GPIO3);
    print_reset_reason();
    let mut store = ScriptStore::new(peripherals.FLASH);

    let spi = init_ttgo_spi(peripherals.SPI2, peripherals.GPIO18, peripherals.GPIO19);
    let cs = make_output_pin(peripherals.GPIO5, Level::High);
    let dc = make_output_pin(peripherals.GPIO16, Level::Low);
    let rst = make_output_pin(peripherals.GPIO23, Level::High);
    let bl = make_output_pin(peripherals.GPIO4, Level::Low);

    let board = Board::new(board_peripherals!(peripherals), TTGO_PINS);

    let mut spi_buf = [0u8; 4096];
    let console = RefCell::new(TftConsole::new(spi, cs, dc, rst, bl, &mut spi_buf));
    let mut out = SharedConsole(&console);
    let mut hw = TtgoHardware {
        board,
        console: &console,
    };

    repl::run(&mut out, &mut hw, &BOOT_SCRIPT, &mut store)
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// Shows why the chip started (power on, watchdog, software reset, ...).
fn print_reset_reason() {
    println!("Reset reason: {:?}", esp_hal::system::reset_reason());
}

/// The embedded boot script, used when no `main.py` is saved in flash.
const BOOT_SCRIPT: BootScript = BootScript {
    source: SCRIPT,
    name: env!("MPY_SCRIPT_NAME"),
};
