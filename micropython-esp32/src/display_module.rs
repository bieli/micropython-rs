//! The `display` module for scripts on the TTGO T-Display:
//!
//! ```python
//! import display
//! display.fill(display.BLACK)
//! display.line(0, 0, display.WIDTH - 1, display.HEIGHT - 1, display.RED)
//! display.fill_circle(120, 67, 30, display.rgb(0, 128, 255))
//! ```
//!
//! Colors are RGB565 integers (like MicroPython's `st7789` drivers): use the
//! constants or `display.rgb(r, g, b)`. The color argument is optional and
//! defaults to `WHITE`. Coordinates may be off the screen; drawing is clipped.

use embedded_graphics::prelude::{Point, Size};
use micropython_rs::{Arg, Error, HwFn, Module};

use crate::tft_console::{color_from_rgb565, rgb565, TftConsole, SCREEN_HEIGHT, SCREEN_WIDTH};

const WHITE: i64 = 0xFFFF;

pub const DISPLAY_MODULE: Module = Module {
    name: "display",
    doc: "Draw on the 240x135 TFT (colors are RGB565 ints)",
    functions: &[
        HwFn::new(
            "fill",
            "color",
            "Fill the screen (also clears the console text)",
        ),
        HwFn::new(
            "backlight",
            "on",
            "Backlight on (1) or off (0): the screen keeps its picture; on again after a script",
        ),
        HwFn::with_args("pixel", 2, 3, "x, y, [color]", "Set one pixel"),
        HwFn::with_args(
            "line",
            4,
            5,
            "x0, y0, x1, y1, [color]",
            "Line between two points",
        ),
        HwFn::with_args("hline", 3, 4, "x, y, w, [color]", "Horizontal line"),
        HwFn::with_args("vline", 3, 4, "x, y, h, [color]", "Vertical line"),
        HwFn::with_args("rect", 4, 5, "x, y, w, h, [color]", "Rectangle outline"),
        HwFn::with_args("fill_rect", 4, 5, "x, y, w, h, [color]", "Filled rectangle"),
        HwFn::with_args(
            "circle",
            3,
            4,
            "x, y, r, [color]",
            "Circle outline around x, y",
        ),
        HwFn::with_args("fill_circle", 3, 4, "x, y, r, [color]", "Filled circle"),
        HwFn::with_args(
            "triangle",
            6,
            7,
            "x0, y0, x1, y1, x2, y2, [color]",
            "Triangle outline",
        ),
        HwFn::with_args(
            "fill_triangle",
            6,
            7,
            "x0, y0, x1, y1, x2, y2, [color]",
            "Filled triangle",
        ),
        HwFn::with_args(
            "text",
            3,
            5,
            "x, y, s, [color], [bg]",
            "Text (6x10 font), top-left at x, y",
        ),
        HwFn::new("rgb", "r, g, b", "RGB565 color from 0-255 components"),
        HwFn::with_args(
            "number",
            3,
            5,
            "x, y, n, [color], [bg]",
            "Draw an integer; returns its width",
        ),
        HwFn::new("font", "size", "Text font: 1 = 6x10, 2 = 8x13, 3 = 10x20"),
        HwFn::new(
            "number_width",
            "n",
            "Width in pixels of n in the current font",
        ),
        HwFn::new(
            "text_width",
            "s",
            "Width in pixels of s in the current font",
        ),
        HwFn::new("font_height", "", "Height in pixels of the current font"),
    ],
    constants: &[
        ("WIDTH", SCREEN_WIDTH as i64),
        ("HEIGHT", SCREEN_HEIGHT as i64),
        ("BLACK", 0x0000),
        ("WHITE", WHITE),
        ("RED", 0xF800),
        ("GREEN", 0x07E0),
        ("BLUE", 0x001F),
        ("YELLOW", 0xFFE0),
        ("CYAN", 0x07FF),
        ("MAGENTA", 0xF81F),
        ("ORANGE", 0xFD20),
        ("GRAY", 0x8410),
    ],
};

/// Coordinates and sizes beyond this are rejected, so a typo like
/// `circle(0, 0, 10**9)` can't keep the board busy for hours.
const MAX_EXTENT: i64 = 4096;

/// Optional background color at `i`, black if missing.
fn background(args: &[Arg<'_>], i: usize) -> Result<embedded_graphics::pixelcolor::Rgb565, Error> {
    let v = args.get(i).map_or(Ok(0), |a| a.int())?;
    u16::try_from(v)
        .map(color_from_rgb565)
        .map_err(|_| Error::ValueError)
}

/// Runs `display.<name>(args)`. The interpreter has already checked the
/// argument count against `DISPLAY_MODULE`.
pub fn call(
    console: &mut TftConsole<'_>,
    name: &str,
    args: &[Arg<'_>],
) -> Result<Option<i64>, Error> {
    let coord = |i: usize| -> Result<i32, Error> {
        let v = args[i].int()?;
        if (-MAX_EXTENT..=MAX_EXTENT).contains(&v) {
            Ok(v as i32)
        } else {
            Err(Error::ValueError)
        }
    };
    let size = |i: usize| -> Result<u32, Error> {
        let v = args[i].int()?;
        if (0..=MAX_EXTENT).contains(&v) {
            Ok(v as u32)
        } else {
            Err(Error::ValueError)
        }
    };
    let point = |i: usize| -> Result<Point, Error> { Ok(Point::new(coord(i)?, coord(i + 1)?)) };
    // Optional color at `i`, white if missing
    let color = |i: usize| -> Result<_, Error> {
        let v = args.get(i).map_or(Ok(WHITE), |a| a.int())?;
        u16::try_from(v)
            .map(color_from_rgb565)
            .map_err(|_| Error::ValueError)
    };
    let component = |i: usize| -> Result<u8, Error> {
        u8::try_from(args[i].int()?).map_err(|_| Error::ValueError)
    };

    match name {
        "fill" => console.fill_screen(color(0)?),
        "backlight" => console.set_backlight(args[0].int()? != 0),
        "pixel" => console.draw_pixel(coord(0)?, coord(1)?, color(2)?),
        "line" => console.draw_line(point(0)?, point(2)?, color(4)?),
        "hline" => {
            let (x, y, w) = (coord(0)?, coord(1)?, size(2)?);
            console.draw_rect(Point::new(x, y), Size::new(w, 1), color(3)?, true);
        }
        "vline" => {
            let (x, y, h) = (coord(0)?, coord(1)?, size(2)?);
            console.draw_rect(Point::new(x, y), Size::new(1, h), color(3)?, true);
        }
        "rect" | "fill_rect" => {
            let size = Size::new(size(2)?, size(3)?);
            console.draw_rect(point(0)?, size, color(4)?, name == "fill_rect");
        }
        "circle" | "fill_circle" => {
            console.draw_circle(point(0)?, size(2)?, color(3)?, name == "fill_circle")
        }
        "triangle" | "fill_triangle" => {
            let corners = [point(0)?, point(2)?, point(4)?];
            console.draw_triangle(corners, color(6)?, name == "fill_triangle");
        }
        "text" => {
            let bg = background(args, 4)?;
            console.draw_text_colored(coord(0)?, coord(1)?, args[2].str()?, color(3)?, bg);
        }
        "number" => {
            let bg = background(args, 4)?;
            let width = console.draw_number(coord(0)?, coord(1)?, args[2].int()?, color(3)?, bg);
            return Ok(Some(width as i64));
        }
        "font" => {
            if !console.set_font(args[0].int()?) {
                return Err(Error::ValueError);
            }
        }
        "number_width" => return Ok(Some(console.number_width(args[0].int()?) as i64)),
        "text_width" => {
            let chars = args[0].str()?.chars().count() as i64;
            return Ok(Some(chars * console.char_width() as i64));
        }
        "font_height" => return Ok(Some(console.font_height() as i64)),
        "rgb" => {
            let value = rgb565(component(0)?, component(1)?, component(2)?);
            return Ok(Some(value as i64));
        }
        _ => return Err(Error::UnknownIdentifier),
    }
    Ok(None)
}
