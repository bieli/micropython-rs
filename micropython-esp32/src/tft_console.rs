use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyleBuilder},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::{
    delay::Delay,
    gpio::{Level, Output as GpioOutput, OutputConfig},
    spi::{
        master::{Config as SpiConfig, Spi},
        Mode,
    },
    time::Rate,
    Blocking,
};
use esp_println::print;
use micropython_rs::Output;
use mipidsi::{
    interface::SpiInterface,
    models::ST7789,
    options::{ColorInversion, Orientation, Rotation},
    Builder, Display,
};

/// Landscape dimensions for the LilyGO TTGO T-Display 1.14" panel
pub const SCREEN_WIDTH: u16 = 240;
pub const SCREEN_HEIGHT: u16 = 135;

/// Character cell dimensions matching `FONT_6X10`
const CHAR_W: i32 = 6;
const CHAR_H: i32 = 12;
const MARGIN_X: i32 = 4;
const MARGIN_Y: i32 = 4;
const MAX_COLS: i32 = (SCREEN_WIDTH as i32 - MARGIN_X * 2) / CHAR_W;   // 38 chars/line
const MAX_ROWS: i32 = (SCREEN_HEIGHT as i32 - MARGIN_Y * 2) / CHAR_H; // 10 lines

/// Concrete type alias for the SPI-backed ST7789 display
pub type TtgoDisplay<'d> = Display<
    SpiInterface<'d, ExclusiveDevice<Spi<'d, Blocking>, GpioOutput<'d>, Delay>, GpioOutput<'d>>,
    ST7789,
    GpioOutput<'d>,
>;

pub struct TftConsole<'d> {
    display: TtgoDisplay<'d>,
    backlight: GpioOutput<'d>,
    col: i32,
    row: i32,
}

impl<'d> TftConsole<'d> {
    /// Initializes SPI2, resets the ST7789 controller, clears the screen, and turns on GPIO4 backlight.
    pub fn new(
        spi: Spi<'d, Blocking>,
        cs: GpioOutput<'d>,
        dc: GpioOutput<'d>,
        rst: GpioOutput<'d>,
        mut backlight: GpioOutput<'d>,
        spi_buffer: &'d mut [u8],
    ) -> Self {
        let spi_device = ExclusiveDevice::new(spi, cs, Delay::new()).unwrap();
        let di = SpiInterface::new(spi_device, dc, spi_buffer);

        let mut delay = Delay::new();
        let mut display = Builder::new(ST7789, di)
            .reset_pin(rst)
            .display_size(135, 240)
            .display_offset(52, 40)
            .invert_colors(ColorInversion::Inverted)
            .orientation(Orientation::new().rotate(Rotation::Deg90))
            .init(&mut delay)
            .unwrap();

        display.clear(Rgb565::BLACK).unwrap();
        backlight.set_high();

        Self {
            display,
            backlight,
            col: 0,
            row: 0,
        }
    }

    pub fn set_backlight(&mut self, on: bool) {
        if on {
            self.backlight.set_high();
        } else {
            self.backlight.set_low();
        }
    }

    pub fn clear(&mut self) {
        let _ = self.display.clear(Rgb565::BLACK);
        self.col = 0;
        self.row = 0;
    }

    fn newline(&mut self) {
        self.col = 0;
        self.row += 1;
        if self.row >= MAX_ROWS {
            // Wrap back to top and clear screen when full
            self.clear();
        }
    }

    /// Draws a single ASCII character at the current cursor cell and advances the cursor.
    pub fn put_char(&mut self, ch: char) {
        if ch == '\n' {
            self.newline();
            return;
        }
        if ch == '\r' {
            self.col = 0;
            return;
        }

        if self.col >= MAX_COLS {
            self.newline();
        }

        let x = MARGIN_X + self.col * CHAR_W;
        let y = MARGIN_Y + self.row * CHAR_H;

        // Clear the character cell background before drawing the glyph
        let _ = Rectangle::new(Point::new(x, y), Size::new(CHAR_W as u32, CHAR_H as u32))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(&mut self.display);

        let mut buf = [0u8; 4];
        let glyph = ch.encode_utf8(&mut buf);

        let style = MonoTextStyleBuilder::new()
            .font(&FONT_6X10)
            .text_color(Rgb565::CSS_LIGHT_GREEN)
            .background_color(Rgb565::BLACK)
            .build();

        let _ = Text::with_baseline(glyph, Point::new(x, y), style, Baseline::Top)
            .draw(&mut self.display);

        self.col += 1;
    }
}

impl<'d> Output for TftConsole<'d> {
    fn write_str(&mut self, s: &str) {
        // Mirror output to USB serial console as well
        print!("{}", s);

        for ch in s.chars() {
            self.put_char(ch);
        }
    }
}

/// Helper to construct SPI2 configured for the TTGO T-Display pins (SCLK=GPIO18, MOSI=GPIO19).
pub fn init_ttgo_spi<'d>(
    spi2: esp_hal::peripherals::SPI2<'d>,
    sclk: esp_hal::peripherals::GPIO18<'d>,
    mosi: esp_hal::peripherals::GPIO19<'d>,
) -> Spi<'d, Blocking> {
    Spi::new(
        spi2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(26))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(sclk)
    .with_mosi(mosi)
}

pub fn make_output_pin<'d>(
    pin: impl esp_hal::gpio::OutputPin + 'd,
    initial: Level,
) -> GpioOutput<'d> {
    GpioOutput::new(pin, initial, OutputConfig::default())
}
