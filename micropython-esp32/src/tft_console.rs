use embedded_graphics::{
    mono_font::{
        ascii::{FONT_10X20, FONT_6X10, FONT_8X13},
        MonoFont, MonoTextStyleBuilder,
    },
    pixelcolor::{raw::RawU16, Rgb565},
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle, StyledDrawable, Triangle},
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
const MAX_COLS: i32 = (SCREEN_WIDTH as i32 - MARGIN_X * 2) / CHAR_W; // 38 chars/line
const MAX_ROWS: i32 = (SCREEN_HEIGHT as i32 - MARGIN_Y * 2) / CHAR_H; // 10 lines
const DEFAULT_COLOR: Rgb565 = Rgb565::CSS_LIGHT_GREEN;
/// Lines to move up when the screen is full. Redrawing the text area costs
/// ~25 ms, so scrolling half a screen at a time keeps long output fast.
const SCROLL_LINES: i32 = MAX_ROWS / 2;

/// The whole screen, used to clip drawing: mipidsi doesn't clip per-pixel
/// drawing itself, so pixels off the screen would wrap around.
const SCREEN: Rectangle = Rectangle::new(
    Point::new(0, 0),
    Size::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32),
);

/// A script color (RGB565 as a 16-bit integer) as an embedded-graphics color.
pub fn color_from_rgb565(value: u16) -> Rgb565 {
    RawU16::new(value).into()
}

/// 8-bit RGB components packed as RGB565, like MicroPython's `color565()`.
pub fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    Rgb565::new(r >> 3, g >> 2, b >> 3).into_storage()
}

/// Decimal text of `n` in `buf` (20 bytes fit any i64).
fn format_i64(n: i64, buf: &mut [u8; 20]) -> &str {
    let mut idx = buf.len();
    let mut v = n.unsigned_abs();
    loop {
        idx -= 1;
        buf[idx] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if n < 0 {
        idx -= 1;
        buf[idx] = b'-';
    }
    core::str::from_utf8(&buf[idx..]).unwrap_or("?")
}

/// One console line is rendered into this RAM strip (240 x 12 pixels,
/// 5.6 KB) and sent to the display in a single SPI transfer, instead of one
/// small transfer per character.
const STRIP_W: usize = SCREEN_WIDTH as usize;
const STRIP_H: usize = CHAR_H as usize;

struct Strip {
    px: [Rgb565; STRIP_W * STRIP_H],
}

impl DrawTarget for Strip {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, color) in pixels {
            if (0..STRIP_W as i32).contains(&p.x) && (0..STRIP_H as i32).contains(&p.y) {
                self.px[p.y as usize * STRIP_W + p.x as usize] = color;
            }
        }
        Ok(())
    }
}

impl OriginDimensions for Strip {
    fn size(&self) -> Size {
        Size::new(STRIP_W as u32, STRIP_H as u32)
    }
}

fn text_style(color: Rgb565) -> embedded_graphics::mono_font::MonoTextStyle<'static, Rgb565> {
    MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(color)
        .background_color(Rgb565::BLACK)
        .build()
}

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
    /// Text of the visible console lines, for redrawing when scrolling.
    lines: [[u8; MAX_COLS as usize]; MAX_ROWS as usize],
    line_lens: [u8; MAX_ROWS as usize],
    /// Color each line was printed in.
    line_colors: [Rgb565; MAX_ROWS as usize],
    color: Rgb565,
    /// The current line changed since it was last drawn.
    dirty: bool,
    strip: Strip,
    /// Font for script text (`display.text`, `display.number`); the console
    /// itself always uses `FONT_6X10`.
    font: &'static MonoFont<'static>,
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
            lines: [[b' '; MAX_COLS as usize]; MAX_ROWS as usize],
            line_lens: [0; MAX_ROWS as usize],
            line_colors: [DEFAULT_COLOR; MAX_ROWS as usize],
            color: DEFAULT_COLOR,
            dirty: false,
            strip: Strip {
                px: [Rgb565::BLACK; STRIP_W * STRIP_H],
            },
            font: &FONT_6X10,
        }
    }

    pub fn set_backlight(&mut self, on: bool) {
        if on {
            self.backlight.set_high();
        } else {
            self.backlight.set_low();
        }
    }

    /// Clears the screen and moves the console cursor to the top.
    pub fn clear(&mut self) {
        let _ = self.display.clear(Rgb565::BLACK);
        self.col = 0;
        self.row = 0;
        self.line_lens = [0; MAX_ROWS as usize];
        self.dirty = false;
    }

    /// Text color for `print()` and `text()`, from 8-bit RGB components.
    pub fn set_color(&mut self, r: u8, g: u8, b: u8) {
        self.color = Rgb565::new(r >> 3, g >> 2, b >> 3);
    }

    /// Draws `s` with its top-left corner at pixel (`x`, `y`), outside the
    /// console's scrolling text. Characters that aren't printable ASCII are
    /// shown as `?`.
    pub fn draw_text(&mut self, x: i32, y: i32, s: &str) {
        self.draw_text_colored(x, y, s, self.color, Rgb565::BLACK);
    }

    /// Selects the font for script text: 1 = 6x10, 2 = 8x13, 3 = 10x20 pixels.
    /// Returns `false` for other sizes.
    pub fn set_font(&mut self, size: i64) -> bool {
        self.font = match size {
            1 => &FONT_6X10,
            2 => &FONT_8X13,
            3 => &FONT_10X20,
            _ => return false,
        };
        true
    }

    /// Width in pixels of one character of the script font.
    pub fn char_width(&self) -> i32 {
        (self.font.character_size.width + self.font.character_spacing) as i32
    }

    pub fn font_height(&self) -> i32 {
        self.font.character_size.height as i32
    }

    /// Draws an integer without needing `str()`; returns its width in pixels.
    pub fn draw_number(&mut self, x: i32, y: i32, n: i64, fg: Rgb565, bg: Rgb565) -> i32 {
        let mut buf = [0u8; 20];
        let text = format_i64(n, &mut buf);
        self.draw_text_colored(x, y, text, fg, bg);
        text.len() as i32 * self.char_width()
    }

    /// Width in pixels of `n` drawn with `draw_number`.
    pub fn number_width(&self, n: i64) -> i32 {
        let mut buf = [0u8; 20];
        format_i64(n, &mut buf).len() as i32 * self.char_width()
    }

    /// `draw_text` with explicit text and background colors.
    pub fn draw_text_colored(&mut self, x: i32, y: i32, s: &str, fg: Rgb565, bg: Rgb565) {
        let style = MonoTextStyleBuilder::new()
            .font(self.font)
            .text_color(fg)
            .background_color(bg)
            .build();
        let mut buf = [0u8; MAX_COLS as usize];
        let mut x = x;
        // Draw in chunks so long strings don't need a big buffer
        for chunk in s.as_bytes().chunks(buf.len()) {
            for (dst, &b) in buf.iter_mut().zip(chunk) {
                *dst = if (0x20..0x7F).contains(&b) { b } else { b'?' };
            }
            let text = core::str::from_utf8(&buf[..chunk.len()]).unwrap_or("");
            let _ = Text::with_baseline(text, Point::new(x, y), style, Baseline::Top)
                .draw(&mut self.display.clipped(&SCREEN));
            x += chunk.len() as i32 * self.char_width();
        }
    }

    /// Fills the whole screen with `color` and moves the console cursor to the top.
    pub fn fill_screen(&mut self, color: Rgb565) {
        let _ = self.display.clear(color);
        self.col = 0;
        self.row = 0;
        self.line_lens = [0; MAX_ROWS as usize];
        self.dirty = false;
    }

    pub fn draw_pixel(&mut self, x: i32, y: i32, color: Rgb565) {
        let _ = Pixel(Point::new(x, y), color).draw(&mut self.display.clipped(&SCREEN));
    }

    pub fn draw_line(&mut self, from: Point, to: Point, color: Rgb565) {
        self.paint(&Line::new(from, to), color, false);
    }

    pub fn draw_rect(&mut self, top_left: Point, size: Size, color: Rgb565, fill: bool) {
        self.paint(&Rectangle::new(top_left, size), color, fill);
    }

    /// A circle around `center`; `radius` 0 is a single pixel.
    pub fn draw_circle(&mut self, center: Point, radius: u32, color: Rgb565, fill: bool) {
        self.paint(&Circle::with_center(center, 2 * radius + 1), color, fill);
    }

    pub fn draw_triangle(&mut self, corners: [Point; 3], color: Rgb565, fill: bool) {
        let [a, b, c] = corners;
        self.paint(&Triangle::new(a, b, c), color, fill);
    }

    /// Draws a shape outlined (1 px) or filled, clipped to the screen.
    fn paint<S>(&mut self, shape: &S, color: Rgb565, fill: bool)
    where
        S: StyledDrawable<PrimitiveStyle<Rgb565>, Color = Rgb565>,
    {
        let style = if fill {
            PrimitiveStyle::with_fill(color)
        } else {
            PrimitiveStyle::with_stroke(color, 1)
        };
        let _ = shape.draw_styled(&style, &mut self.display.clipped(&SCREEN));
    }

    /// Fills a rectangle in the current color (`fill == false` clears it to black).
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, fill: bool) {
        let color = if fill { self.color } else { Rgb565::BLACK };
        let _ = Rectangle::new(Point::new(x, y), Size::new(w, h))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(&mut self.display);
    }

    fn newline(&mut self) {
        self.flush_line();
        self.col = 0;
        self.row += 1;
        if self.row >= MAX_ROWS {
            self.scroll();
        }
        self.line_lens[self.row as usize] = 0;
    }

    /// Moves the console lines up by `SCROLL_LINES` and redraws them. Only
    /// the text area is repainted, so graphics drawn below it (status bars) stay.
    fn scroll(&mut self) {
        let n = SCROLL_LINES as usize;
        self.lines.copy_within(n.., 0);
        self.line_lens.copy_within(n.., 0);
        self.line_colors.copy_within(n.., 0);
        self.row = MAX_ROWS - SCROLL_LINES;
        for len in &mut self.line_lens[self.row as usize..] {
            *len = 0;
        }
        for row in 0..MAX_ROWS {
            self.render_row(row);
        }
    }

    /// Draws the current line if it changed.
    fn flush_line(&mut self) {
        if self.dirty {
            self.render_row(self.row);
            self.dirty = false;
        }
    }

    /// Renders one console line into the RAM strip and sends it to the
    /// display in one transfer.
    fn render_row(&mut self, row: i32) {
        let len = self.line_lens[row as usize] as usize;
        self.strip.px.fill(Rgb565::BLACK);
        // Lines only hold printable ASCII (see `put_char`)
        let text = core::str::from_utf8(&self.lines[row as usize][..len]).unwrap_or("");
        let _ = Text::with_baseline(
            text,
            Point::new(MARGIN_X, 0),
            text_style(self.line_colors[row as usize]),
            Baseline::Top,
        )
        .draw(&mut self.strip);

        let area = Rectangle::new(
            Point::new(0, MARGIN_Y + row * CHAR_H),
            Size::new(STRIP_W as u32, STRIP_H as u32),
        );
        let _ = self
            .display
            .fill_contiguous(&area, self.strip.px.iter().copied());
    }

    /// Adds one character at the cursor. The line is drawn on the next
    /// newline or at the end of `write_str`.
    pub fn put_char(&mut self, ch: char) {
        if ch == '\n' {
            self.newline();
            return;
        }
        if ch == '\r' {
            self.col = 0;
            return;
        }
        if ch == '\x08' {
            // Backspace (the REPL erases with "\x08 \x08")
            self.col = (self.col - 1).max(0);
            return;
        }

        if self.col >= MAX_COLS {
            self.newline();
        }

        let ch = if ch.is_ascii() && !ch.is_ascii_control() {
            ch
        } else {
            '?'
        };
        let row = self.row as usize;
        self.lines[row][self.col as usize] = ch as u8;
        self.col += 1;
        self.line_lens[row] = self.line_lens[row].max(self.col as u8);
        self.line_colors[row] = self.color;
        self.dirty = true;
    }
}

impl<'d> Output for TftConsole<'d> {
    fn write_str(&mut self, s: &str) {
        // Mirror output to USB serial console as well
        crate::serial_console::write_serial(s);

        for ch in s.chars() {
            self.put_char(ch);
        }
        self.flush_line();
    }
}

/// `print()` output that goes to a console shared with the hardware
/// functions (`cls()`, `text()`, ...), which need it at other times.
pub struct SharedConsole<'a, 'd>(pub &'a core::cell::RefCell<TftConsole<'d>>);

impl Output for SharedConsole<'_, '_> {
    fn write_str(&mut self, s: &str) {
        self.0.borrow_mut().write_str(s);
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
            .with_frequency(Rate::from_mhz(40))
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
