//! Pins by number, as scripts give them: `port * 16 + pin` (PA0 is 0, PB0 16,
//! PC0 32, PD0 48), and the XIAO's labels as constants (`D0` is PC0 = 32,
//! `LED` is PA7 = 7). Registers directly (mg24-hal's pins are types, one per
//! pin, chosen at compile time).
//!
//! Pins the board needs for itself are refused (`ValueError`): PA1/PA2 (SWD:
//! the debugger that flashes the board), PA6 (the external flash's chip
//! select), PA8/PA9 (the console; the XIAO's D12/D11 pads share them), PB4/PB5
//! (the radio's antenna switch), PD3/PD4 (battery control and sense).

use micropython_rs::Error;

use efr32mg24_pac as pac;

/// `pin_mode` modes, as on the ESP32 boards (`machine.py` uses them).
pub const OUT: i64 = 1;
pub const IN: i64 = 2;
pub const IN_PULLUP: i64 = 3;
pub const IN_PULLDOWN: i64 = 4;
pub const OUT_OD: i64 = 5;
pub const OUT_OD_PULLUP: i64 = 6;

/// GPIO_Px_MODEL/MODEH values (4 bits per pin).
const MODE_INPUT: u32 = 1;
const MODE_INPUTPULL: u32 = 2; // the pull's direction is DOUT
const MODE_PUSHPULL: u32 = 4;
const MODE_WIREDAND: u32 = 8; // open drain
const MODE_WIREDANDPULLUP: u32 = 10;

/// Pins per port on this package (QFN48): PA0-PA9, PB0-PB5, PC0-PC9, PD0-PD5.
const PORT_PINS: [u8; 4] = [10, 6, 10, 6];
/// (port, pin) the board uses itself.
const RESERVED: [(u8, u8); 9] = [
    (0, 1),
    (0, 2),
    (0, 6),
    (0, 8),
    (0, 9),
    (1, 4),
    (1, 5),
    (3, 3),
    (3, 4),
];

/// The XIAO's pin labels: D0-D18 and the LED, as pin numbers.
pub const LABELS: [(&str, i64); 20] = [
    ("D0", 32),
    ("D1", 33),
    ("D2", 34),
    ("D3", 35),
    ("D4", 36),
    ("D5", 37),
    ("D6", 38),
    ("D7", 39),
    ("D8", 3),
    ("D9", 4),
    ("D10", 5),
    ("D11", 9),
    ("D12", 8),
    ("D13", 18),
    ("D14", 19),
    ("D15", 16),
    ("D16", 17),
    ("D17", 0),
    ("D18", 50),
    ("LED", 7),
];

/// The default I2C pins (`machine.I2C()` without pins): D4 (PC4) and D5 (PC5).
pub const I2C_SDA: i64 = 36;
pub const I2C_SCL: i64 = 37;

/// A pin the scripts may use: (port, pin).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PinId {
    pub port: u8,
    pub pin: u8,
}

impl PinId {
    /// The pin `n` (`port * 16 + pin`), if it exists and is free.
    pub fn new(n: i64) -> Result<Self, Error> {
        let (port, pin) = (n / 16, n % 16);
        if !(0..4).contains(&port) || pin >= PORT_PINS[port as usize] as i64 {
            return Err(Error::ValueError);
        }
        let id = PinId {
            port: port as u8,
            pin: pin as u8,
        };
        if RESERVED.contains(&(id.port, id.pin)) {
            return Err(Error::ValueError);
        }
        Ok(id)
    }
}

/// A port's registers, from the PAC's port A ones and the stride between ports.
struct Port {
    model: *mut u32,
    modeh: *mut u32,
    dout: *mut u32,
    din: *const u32,
}

fn port(n: u8) -> Port {
    // SAFETY: GPIO's registers (only this module and the routes' setup use them)
    let gpio = unsafe { &*pac::GpioS::ptr() };
    let stride = gpio.portb_model() as *const _ as usize - gpio.porta_model() as *const _ as usize;
    let at = |reg: usize| (reg + n as usize * stride) as *mut u32;
    Port {
        model: at(gpio.porta_model() as *const _ as usize),
        modeh: at(gpio.porta_modeh() as *const _ as usize),
        dout: at(gpio.porta_dout() as *const _ as usize),
        din: at(gpio.porta_din() as *const _ as usize),
    }
}

/// Sets a pin's 4-bit mode and its DOUT bit (the pull direction for inputs).
pub fn set_mode(id: PinId, mode: u32, dout: bool) {
    let p = port(id.port);
    let (reg, shift) = if id.pin < 8 {
        (p.model, id.pin * 4)
    } else {
        (p.modeh, (id.pin - 8) * 4)
    };
    // SAFETY: the port's registers; only the main code changes them (no
    // interrupt handler does), so read-modify-write is safe
    unsafe {
        write(id, dout);
        let v = reg.read_volatile() & !(0xF << shift) | mode << shift;
        reg.write_volatile(v);
    }
}

/// `pin_mode(pin, mode)` with the ESP32 boards' modes.
pub fn pin_mode(n: i64, mode: i64) -> Result<(), Error> {
    let id = PinId::new(n)?;
    let (m, dout) = match mode {
        OUT => (MODE_PUSHPULL, read_dout(id)),
        IN => (MODE_INPUT, false),
        IN_PULLUP => (MODE_INPUTPULL, true),
        IN_PULLDOWN => (MODE_INPUTPULL, false),
        OUT_OD => (MODE_WIREDAND, read_dout(id)),
        OUT_OD_PULLUP => (MODE_WIREDANDPULLUP, read_dout(id)),
        _ => return Err(Error::ValueError),
    };
    set_mode(id, m, dout);
    Ok(())
}

/// A push-pull output at `high` (a PWM, UART or SPI output).
pub fn set_push_pull(id: PinId, high: bool) {
    set_mode(id, MODE_PUSHPULL, high);
}

/// Disconnected (mode DISABLED): a pin a peripheral gave back.
pub fn release(id: PinId) {
    set_mode(id, 0, false);
}

/// An input with the pull-up (a UART or SPI input).
pub fn input_pullup(id: PinId) {
    set_mode(id, MODE_INPUTPULL, true);
}

/// Open drain with the pull-up, high (an I2C line).
pub fn open_drain_pullup(id: PinId) {
    set_mode(id, MODE_WIREDANDPULLUP, true);
}

fn read_dout(id: PinId) -> bool {
    // SAFETY: reads the port's DOUT
    unsafe { port(id.port).dout.read_volatile() >> id.pin & 1 != 0 }
}

/// Sets the pin's output level (DOUT).
pub fn write(id: PinId, high: bool) {
    let p = port(id.port);
    // SAFETY: as in `set_mode`
    unsafe {
        let v = p.dout.read_volatile();
        let bit = 1 << id.pin;
        p.dout.write_volatile(if high { v | bit } else { v & !bit });
    }
}

/// Inverts the pin's output level.
pub fn toggle(id: PinId) {
    let p = port(id.port);
    // SAFETY: as in `set_mode`
    unsafe { p.dout.write_volatile(p.dout.read_volatile() ^ 1 << id.pin) };
}

/// The pin's level (DIN; for an output, the level on the pad).
pub fn read(id: PinId) -> bool {
    // SAFETY: reads the port's DIN
    unsafe { port(id.port).din.read_volatile() >> id.pin & 1 != 0 }
}

/// For `bench`: the DOUT register and the pin's bit, to toggle in a loop.
pub fn dout_reg(id: PinId) -> (*mut u32, u32) {
    (port(id.port).dout, 1 << id.pin)
}

/// For `bench`: the DIN register and the pin's bit.
pub fn din_reg(id: PinId) -> (*const u32, u32) {
    (port(id.port).din, 1 << id.pin)
}
