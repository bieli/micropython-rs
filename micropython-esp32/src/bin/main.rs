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
use micropython_esp32::buttons::{Buttons, BUTTONS_MODULE};
use micropython_esp32::repl::{self, BootScript};
use micropython_esp32::serial_input;
use micropython_esp32::storage;
use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Module, Returns};

esp_bootloader_esp_idf::esp_app_desc!();

/// The Python script to run, chosen at build time by `build.rs`:
/// `make flash SCRIPT=scripts/blink.py`, or the board's demo from `scripts/`.
const SCRIPT: &str = include_str!(env!("MPY_SCRIPT_PATH"));

/// Takes the peripherals `Board` needs out of `esp_hal::init`'s result.
macro_rules! board_peripherals {
    ($p:ident) => {
        BoardPeripherals {
            io_mux: $p.IO_MUX,
            #[cfg(feature = "wifi")]
            wifi: $p.WIFI,
            #[cfg(feature = "wifi")]
            timg1: $p.TIMG1,
            #[cfg(all(feature = "wifi", feature = "esp32c3"))]
            sw_int0: esp_hal::interrupt::software::SoftwareInterruptControl::new($p.SW_INTERRUPT)
                .software_interrupt0,
            adc1: $p.ADC1,
            #[cfg(feature = "esp32")]
            adc_a: $p.GPIO34,
            #[cfg(feature = "esp32")]
            adc_b: $p.GPIO36,
            #[cfg(feature = "esp32")]
            adc_c: $p.GPIO39,
            #[cfg(feature = "esp32s3")]
            adc_a: $p.GPIO1,
            #[cfg(feature = "esp32s3")]
            adc_b: $p.GPIO2,
            #[cfg(feature = "esp32s3")]
            adc_c: $p.GPIO3,
            #[cfg(feature = "esp32c3")]
            adc_a: $p.GPIO0,
            #[cfg(feature = "esp32c3")]
            adc_b: $p.GPIO1,
            #[cfg(feature = "esp32c3")]
            adc_c: $p.GPIO3,
            ledc: $p.LEDC,
            i2c0: $p.I2C0,
            #[cfg(feature = "esp32")]
            sda: $p.GPIO21,
            #[cfg(feature = "esp32")]
            scl: $p.GPIO22,
            #[cfg(feature = "esp32s3")]
            sda: $p.GPIO5,
            #[cfg(feature = "esp32s3")]
            scl: $p.GPIO6,
            #[cfg(feature = "esp32c3")]
            sda: $p.GPIO6,
            #[cfg(feature = "esp32c3")]
            scl: $p.GPIO7,
            lpwr: $p.LPWR,
        }
    };
}

// ---------------------------------------------------------------------------
// Generic ESP32 board (no display)
// ---------------------------------------------------------------------------

/// Pins scripts may use: everything except flash (6-11), UART0 (1, 3),
/// I2C (21, 22) and the ADC inputs (34, 36, 39).
#[cfg(all(feature = "esp32", not(feature = "tft")))]
const GENERIC_PINS: &[u8] = &[
    0, 2, 4, 5, 12, 13, 14, 15, 16, 17, 18, 19, 23, 25, 26, 27, 32, 33, 35,
];

/// ESP32-S3: everything except the ADC inputs (1-3), I2C (5, 6), USB (19,
/// 20) and the flash and PSRAM pins (26-37, octal PSRAM as on the XIAO's
/// module). The XIAO ESP32-S3's header: D0-D2 = GPIO1-3 (ADC), D3 = 4,
/// D4/D5 = 5/6 (I2C), D6/D7 = 43/44, D8-D10 = 7-9; its LED is GPIO21.
#[cfg(feature = "esp32s3")]
const GENERIC_PINS: &[u8] = &[
    0, 4, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 21, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
    48,
];

/// ESP32-C3: everything except the ADC inputs (0, 1, 3), I2C (6, 7), flash
/// (11-17) and USB (18, 19): the ESP32-C3 Super Mini's header pins 2, 4, 5,
/// 8 (its LED), 9 (its BOOT button), 10, 20 and 21.
#[cfg(feature = "esp32c3")]
const GENERIC_PINS: &[u8] = &[2, 4, 5, 8, 9, 10, 20, 21];

/// Most ESP32 DevKit boards wire the onboard LED to GPIO2; the XIAO ESP32-S3's
/// user LED is GPIO21, the ESP32-C3 Super Mini's GPIO8, both lit when low.
#[cfg(all(feature = "esp32", not(feature = "tft")))]
const LED_PIN: i64 = 2;
#[cfg(feature = "esp32s3")]
const LED_PIN: i64 = 21;
#[cfg(feature = "esp32c3")]
const LED_PIN: i64 = 8;
#[cfg(not(feature = "tft"))]
const LED_ON_LEVEL: i64 = if cfg!(feature = "esp32") { 1 } else { 0 };
/// The BOOT button, LEFT of the `buttons` module (GPIO9 on the ESP32-C3)
#[cfg(not(feature = "tft"))]
const BOOT_PIN: u8 = if cfg!(feature = "esp32c3") { 9 } else { 0 };

#[cfg(all(feature = "esp32", not(feature = "tft")))]
const GENERIC_FNS: &[HwFn] = micropython_esp32::board_functions!(
    HwFn::new("led_on", "", "Onboard LED on (GPIO2)"),
    HwFn::new("led_off", "", "Onboard LED off"),
);
#[cfg(feature = "esp32s3")]
const GENERIC_FNS: &[HwFn] = micropython_esp32::board_functions!(
    HwFn::new("led_on", "", "Onboard LED on (GPIO21 on the XIAO ESP32-S3)"),
    HwFn::new("led_off", "", "Onboard LED off"),
);
#[cfg(feature = "esp32c3")]
const GENERIC_FNS: &[HwFn] = micropython_esp32::board_functions!(
    HwFn::new(
        "led_on",
        "",
        "Onboard LED on (GPIO8 on the ESP32-C3 Super Mini)"
    ),
    HwFn::new("led_off", "", "Onboard LED off"),
);

#[cfg(not(feature = "tft"))]
const GENERIC_CONSTS: &[(&str, i64)] =
    micropython_esp32::board_constants!(("LED", LED_PIN), ("BOOT_BUTTON", BOOT_PIN as i64));

#[cfg(not(feature = "tft"))]
struct GenericHardware<'d> {
    board: Board<'d>,
    buttons: Buttons,
}

#[cfg(not(feature = "tft"))]
impl HardwareBridge for GenericHardware<'_> {
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        match name {
            "led_on" => self
                .board
                .call("pin_write", &[Arg::Int(LED_PIN), Arg::Int(LED_ON_LEVEL)]),
            "led_off" => self.board.call(
                "pin_write",
                &[Arg::Int(LED_PIN), Arg::Int(1 - LED_ON_LEVEL)],
            ),
            _ => self.board.call(name, args),
        }
    }

    fn call_bytes(&mut self, name: &str, args: &[Arg<'_>], out: &mut [u8]) -> Result<usize, Error> {
        self.board.call_bytes(name, args, out)
    }

    fn call_values(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[Arg<'_>],
        out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        self.board.call_values(module, name, args, out)
    }

    fn poll_event(&mut self) -> Option<i64> {
        self.board.poll_event()
    }

    fn set_events(&mut self, on: bool) {
        self.board.set_events(on)
    }

    fn resume_sleep(&mut self, remaining_ms: u32) -> Result<(), Error> {
        self.board.resume_sleep(remaining_ms)
    }

    fn soft_reset(&mut self) {
        self.board.soft_reset()
    }

    fn idle(&mut self) {
        self.board.idle()
    }

    fn functions(&self) -> &[HwFn] {
        GENERIC_FNS
    }

    fn constants(&self) -> &[(&'static str, i64)] {
        GENERIC_CONSTS
    }

    fn modules(&self) -> &[Module] {
        &[TIME_MODULE, BUTTONS_MODULE]
    }

    fn call_module(
        &mut self,
        module: &str,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        match module {
            "buttons" => self.buttons.call(&mut self.board, name, args),
            _ => self.board.call_module(module, name, args),
        }
    }
}

#[cfg(not(feature = "tft"))]
#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(micropython_esp32::board::CPU_CLOCK);
    let peripherals = esp_hal::init(config);
    #[cfg(feature = "wifi")]
    wifi_heap();
    // Set up serial input first: it resets UART0, which drops unsent output
    #[cfg(feature = "esp32")]
    serial_input::init(peripherals.UART0, peripherals.GPIO3);
    #[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
    serial_input::init(peripherals.USB_DEVICE);
    print_reset_reason();
    // Shared by the REPL's file commands and the scripts (`open()`, `os`)
    let files = storage::mount(peripherals.FLASH).map(core::cell::RefCell::new);

    #[cfg(feature = "psram")]
    let big_heap = psram_heap(&peripherals.PSRAM);
    #[cfg(not(feature = "psram"))]
    let big_heap = None;
    let mut board = Board::new(board_peripherals!(peripherals), GENERIC_PINS);
    // Most DevKits only have the BOOT button (GPIO0, GPIO9 on the C3) as a
    // user button: LEFT
    let buttons = Buttons::new(&mut board, Some((BOOT_PIN, true)), None);
    let mut out = micropython_esp32::serial_console::EspSerialOut;
    let mut hw = GenericHardware { board, buttons };

    // From here on, `stack_peak()` can tell how deep the stack went
    micropython_esp32::stack::paint();
    repl::run(&mut out, &mut hw, &BOOT_SCRIPT, files.as_ref(), big_heap)
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
    HwFn::with_args(
        "battery_mv",
        0,
        1,
        "[samples]",
        "Battery voltage in mV (GPIO34 divider)"
    ),
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
    buttons: Buttons,
    console: &'a RefCell<TftConsole<'d>>,
}

/// GPIO14 connects the TTGO's battery voltage divider (2 x 100 kOhm) to GPIO34.
#[cfg(feature = "tft")]
const TTGO_ADC_EN: u8 = 14;
#[cfg(feature = "tft")]
const TTGO_BAT_ADC: i64 = 34;

#[cfg(feature = "tft")]
impl TtgoHardware<'_, '_> {
    /// Battery voltage: the divider halves it, so the ADC reading is doubled.
    /// The divider is only switched on while measuring, so it doesn't drain
    /// the battery.
    fn battery_mv(&mut self, samples: i64) -> Result<i64, Error> {
        let on = [Arg::Int(TTGO_ADC_EN as i64), Arg::Int(1)];
        self.board.call("pin_write", &on)?;
        self.board.sleep_ms(2)?;
        let result = self.board.adc_mv(TTGO_BAT_ADC, samples);
        let off = [Arg::Int(TTGO_ADC_EN as i64), Arg::Int(0)];
        self.board.call("pin_write", &off)?;
        Ok(result? * 2)
    }
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
            "battery_mv" => {
                drop(console);
                let samples = match args.first() {
                    Some(arg) => arg.int()?,
                    None => 16,
                };
                return self.battery_mv(samples).map(Some);
            }
            _ => {
                drop(console);
                return self.board.call(name, args);
            }
        }
        Ok(None)
    }

    fn call_bytes(&mut self, name: &str, args: &[Arg<'_>], out: &mut [u8]) -> Result<usize, Error> {
        self.board.call_bytes(name, args, out)
    }

    fn call_values(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[Arg<'_>],
        out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        self.board.call_values(module, name, args, out)
    }

    fn poll_event(&mut self) -> Option<i64> {
        self.board.poll_event()
    }

    fn set_events(&mut self, on: bool) {
        self.board.set_events(on)
    }

    fn resume_sleep(&mut self, remaining_ms: u32) -> Result<(), Error> {
        self.board.resume_sleep(remaining_ms)
    }

    fn soft_reset(&mut self) {
        self.board.soft_reset();
        // A script may have switched it off (display.backlight(0))
        self.console.borrow_mut().set_backlight(true);
    }

    fn idle(&mut self) {
        self.board.idle()
    }

    fn functions(&self) -> &[HwFn] {
        TTGO_FNS
    }

    fn constants(&self) -> &[(&'static str, i64)] {
        TTGO_CONSTS
    }

    fn modules(&self) -> &[Module] {
        &[TIME_MODULE, DISPLAY_MODULE, BUTTONS_MODULE]
    }

    fn call_module(
        &mut self,
        module: &str,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        match module {
            "display" => display_module::call(&mut self.console.borrow_mut(), name, args),
            "buttons" => self.buttons.call(&mut self.board, name, args),
            _ => self.board.call_module(module, name, args),
        }
    }
}

#[cfg(feature = "tft")]
#[main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(micropython_esp32::board::CPU_CLOCK);
    let peripherals = esp_hal::init(config);
    #[cfg(feature = "wifi")]
    wifi_heap();
    // Set up serial input first: it resets UART0, which drops unsent output
    serial_input::init(peripherals.UART0, peripherals.GPIO3);
    print_reset_reason();
    // Shared by the REPL's file commands and the scripts (`open()`, `os`)
    let files = storage::mount(peripherals.FLASH).map(core::cell::RefCell::new);

    let spi = init_ttgo_spi(peripherals.SPI2, peripherals.GPIO18, peripherals.GPIO19);
    let cs = make_output_pin(peripherals.GPIO5, Level::High);
    let dc = make_output_pin(peripherals.GPIO16, Level::Low);
    let rst = make_output_pin(peripherals.GPIO23, Level::High);
    let bl = make_output_pin(peripherals.GPIO4, Level::Low);

    let mut board = Board::new(board_peripherals!(peripherals), TTGO_PINS);
    // TTGO buttons: GPIO0 (left, internal pull-up) and GPIO35 (right, external pull-up)
    let buttons = Buttons::new(&mut board, Some((0, true)), Some((35, false)));

    let mut spi_buf = [0u8; 4096];
    let console = RefCell::new(TftConsole::new(spi, cs, dc, rst, bl, &mut spi_buf));
    let mut out = SharedConsole(&console);
    let mut hw = TtgoHardware {
        board,
        buttons,
        console: &console,
    };

    // From here on, `stack_peak()` can tell how deep the stack went
    micropython_esp32::stack::paint();
    repl::run(&mut out, &mut hw, &BOOT_SCRIPT, files.as_ref(), None)
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// Shows why the chip started (power on, watchdog, software reset, ...).
fn print_reset_reason() {
    println!("Reset reason: {:?}", esp_hal::system::reset_reason());
}

/// The interpreter's heap in PSRAM (`psram` feature): all of it but the
/// object table at its start (65 535 entries, the most an object handle
/// addresses: 768 KB). `None` without PSRAM (the internal 32 KB heap then).
#[cfg(feature = "psram")]
fn psram_heap(psram: &esp_hal::peripherals::PSRAM<'_>) -> Option<repl::BigHeap> {
    use micropython_rs::heap::HeapEntry;
    let (start, size) = esp_hal::psram::psram_raw_parts(psram);
    micropython_esp32::PSRAM_SIZE.store(size as u32, core::sync::atomic::Ordering::Relaxed);
    let entries = u16::MAX as usize;
    let table_bytes = entries * core::mem::size_of::<HeapEntry>();
    if size < table_bytes + 64 * 1024 {
        return None;
    }
    let table = start as *mut HeapEntry;
    for i in 0..entries {
        // SAFETY: inside the PSRAM range esp-hal mapped; written before use
        unsafe { table.add(i).write(HeapEntry::FREE) };
    }
    // SAFETY: the PSRAM is ours alone (nothing else allocates from it); the
    // table and the heap don't overlap and live as long as the firmware
    unsafe {
        Some(repl::BigHeap {
            table: core::slice::from_raw_parts_mut(table, entries),
            heap: core::slice::from_raw_parts_mut(start.add(table_bytes), size - table_bytes),
        })
    }
}

/// The embedded boot script, used when no `main.py` is saved in flash.
const BOOT_SCRIPT: BootScript = BootScript {
    source: SCRIPT,
    name: env!("MPY_SCRIPT_NAME"),
};

/// The heap the Wi-Fi driver allocates from (`wifi` feature): 16 KB in the
/// second data RAM region (what the REPL's buffers leave of it) and 56 KB of
/// the main data RAM, which the stack gives up. With the station and the
/// access point busy, the driver used up to about 60 KB.
///
/// ESP32: 52 KB of main RAM (68 KB in all, the driver's most seen was 55 KB):
/// the 4 KB less than before hold the REPL's statement cache, so the stack
/// keeps the 102 KB that starting the REPL needs.
#[cfg(all(feature = "wifi", feature = "esp32"))]
fn wifi_heap() {
    esp_alloc::heap_allocator!(#[link_section = ".dram2_uninit"] size: 16 * 1024);
    esp_alloc::heap_allocator!(size: 52 * 1024);
}

/// ESP32-C3: 64 KB in `dram2` (66 KB there) and 8 KB of the main data RAM.
#[cfg(all(feature = "wifi", feature = "esp32c3"))]
fn wifi_heap() {
    esp_alloc::heap_allocator!(#[link_section = ".dram2_uninit"] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 8 * 1024);
}

/// ESP32-S3: all 72 KB in `dram2` (72.0 KB there; the REPL's buffers use
/// the main data RAM on this chip), so the stack keeps its size.
#[cfg(all(feature = "wifi", feature = "esp32s3"))]
fn wifi_heap() {
    esp_alloc::heap_allocator!(#[link_section = ".dram2_uninit"] size: 72 * 1024);
}
