//! The XIAO MG24's functions for scripts (`HardwareBridge`): the same names,
//! arguments and constants as micropython-esp32's (pin_mode, pin_write,
//! i2c_read, sleep_ms, cpu_info, ...), so the frozen `machine` and `time`
//! modules from micropython-esp32/modules run unchanged and scripts move
//! between the boards. ADC, PWM, UART, SPI, the watchdog and pin interrupts
//! come later; until then those `machine` classes raise NameError.

use core::sync::atomic::Ordering;

use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Module, Returns};

use crate::clock;
use crate::gpio::{self, PinId};
use crate::i2c::I2c;
use efr32mg24_pac as pac;

pub const PWRON_RESET: i64 = 1;
pub const HARD_RESET: i64 = 2;
pub const WDT_RESET: i64 = 3;
pub const DEEPSLEEP_RESET: i64 = 4;
pub const SOFT_RESET: i64 = 5;

/// The orange user LED: PA7, lit when the pin is low.
const LED_PIN: i64 = 7;
const LED_ON_LEVEL: bool = false;
/// The default I2C speed (machine.I2C asks for 400 kHz itself).
const I2C_DEFAULT_HZ: u32 = 100_000;

const FUNCTIONS: &[HwFn] = &[
    HwFn::new("sleep_ms", "ms", "Wait ms milliseconds"),
    HwFn::new("sleep_us", "us", "Wait us microseconds"),
    HwFn::new("delay_ms", "ms", "Same as sleep_ms"),
    HwFn::new("ticks_ms", "", "Milliseconds since boot"),
    HwFn::new("ticks_us", "", "Microseconds since boot"),
    HwFn::new("ticks_diff", "new, old", "new - old"),
    HwFn::new(
        "pin_mode",
        "pin, mode",
        "mode: OUT, IN, IN_PULLUP, IN_PULLDOWN, OUT_OD, OUT_OD_PULLUP",
    ),
    HwFn::new("pin_write", "pin, value", "Set an output pin to 0 or 1"),
    HwFn::new("pin_read", "pin", "Read a pin: 0 or 1"),
    HwFn::new("pin_toggle", "pin", "Invert an output pin"),
    HwFn::new("led_on", "", "The orange LED on (PA7)"),
    HwFn::new("led_off", "", "The orange LED off"),
    HwFn::with_args(
        "i2c_init",
        2,
        3,
        "sda, scl, [freq]",
        "Move the I2C bus (default D4, D5 at 100 kHz)",
    ),
    HwFn::new("i2c_probe", "addr", "True if a device answers at addr"),
    HwFn::new(
        "i2c_scan",
        "",
        "Addresses of the devices on the bus (bytes)",
    )
    .returning_bytes(),
    HwFn::new("i2c_write", "addr, data", "Send bytes; returns how many"),
    HwFn::new("i2c_read", "addr, n", "Receive n bytes").returning_bytes(),
    HwFn::new(
        "i2c_write_read",
        "addr, data, n",
        "Send, then receive n bytes (e.g. a register)",
    )
    .returning_bytes(),
    HwFn::new("i2c_read_reg", "addr, reg", "Read one register byte"),
    HwFn::new(
        "i2c_read_reg16",
        "addr, reg",
        "Read a big-endian 16-bit register",
    ),
    HwFn::new(
        "i2c_write_reg",
        "addr, reg, value",
        "Write one register byte",
    ),
    HwFn::new("reset", "", "Restart the board"),
    HwFn::new(
        "reset_cause",
        "",
        "PWRON_RESET, HARD_RESET, WDT_RESET, SOFT_RESET, ...",
    ),
    HwFn::new("unique_id", "", "The chip's EUI-64 (8 bytes)").returning_bytes(),
    HwFn::new("cpu_freq", "", "CPU clock in Hz"),
    HwFn::with_args(
        "rtc_set",
        1,
        2,
        "seconds, [microseconds]",
        "Set the clock: seconds since 2000-01-01 (UTC)",
    ),
    HwFn::new(
        "cpu_info",
        "",
        "Chip, CPU, memory and build details as a dict (machine.lscpu())",
    )
    .returning_values(),
    HwFn::with_args(
        "cpu_bench",
        1,
        2,
        "name, [pin]",
        "A native speed test: operations, bytes or toggles per second (use machine.bench())",
    ),
];

/// MicroPython's `utime` (the frozen `time` module adds `mktime` on top), as
/// on the ESP32 boards.
const TIME_MODULE: Module = Module {
    name: "utime",
    doc: "Delays, timestamps and the clock (MicroPython names)",
    functions: &[
        HwFn::new("sleep", "s", "Wait s seconds (a float: 0.5)"),
        HwFn::new("sleep_ms", "ms", "Wait ms milliseconds"),
        HwFn::new("sleep_us", "us", "Wait us microseconds"),
        HwFn::new("ticks_ms", "", "Milliseconds since boot"),
        HwFn::new("ticks_us", "", "Microseconds since boot"),
        HwFn::new("ticks_diff", "new, old", "new - old"),
        HwFn::new("ticks_add", "ticks, delta", "ticks + delta"),
        HwFn::new("time", "", "Seconds since 2000-01-01 (UTC), from the clock"),
        HwFn::new("time_ns", "", "Nanoseconds since 2000-01-01 (UTC)"),
        HwFn::with_args(
            "gmtime",
            0,
            1,
            "[secs]",
            "(year, month, mday, hour, minute, second, weekday, yearday)",
        )
        .returning_values(),
        HwFn::with_args(
            "localtime",
            0,
            1,
            "[secs]",
            "The same as gmtime: the board has no time zone",
        )
        .returning_values(),
        HwFn::new(
            "_mktime",
            "year, month, mday, hour, minute, second",
            "Seconds since 2000 (time.mktime takes the tuple)",
        ),
    ],
    constants: &[],
};

const CONSTANTS: &[(&str, i64)] = &[
    ("OUT", gpio::OUT),
    ("IN", gpio::IN),
    ("IN_PULLUP", gpio::IN_PULLUP),
    ("IN_PULLDOWN", gpio::IN_PULLDOWN),
    ("OUT_OD", gpio::OUT_OD),
    ("OUT_OD_PULLUP", gpio::OUT_OD_PULLUP),
    ("HIGH", 1),
    ("LOW", 0),
    ("PWRON_RESET", PWRON_RESET),
    ("HARD_RESET", HARD_RESET),
    ("WDT_RESET", WDT_RESET),
    ("DEEPSLEEP_RESET", DEEPSLEEP_RESET),
    ("SOFT_RESET", SOFT_RESET),
    ("I2C_SDA", gpio::I2C_SDA),
    ("I2C_SCL", gpio::I2C_SCL),
    ("D0", gpio::LABELS[0].1),
    ("D1", gpio::LABELS[1].1),
    ("D2", gpio::LABELS[2].1),
    ("D3", gpio::LABELS[3].1),
    ("D4", gpio::LABELS[4].1),
    ("D5", gpio::LABELS[5].1),
    ("D6", gpio::LABELS[6].1),
    ("D7", gpio::LABELS[7].1),
    ("D8", gpio::LABELS[8].1),
    ("D9", gpio::LABELS[9].1),
    ("D10", gpio::LABELS[10].1),
    ("D11", gpio::LABELS[11].1),
    ("D12", gpio::LABELS[12].1),
    ("D13", gpio::LABELS[13].1),
    ("D14", gpio::LABELS[14].1),
    ("D15", gpio::LABELS[15].1),
    ("D16", gpio::LABELS[16].1),
    ("D17", gpio::LABELS[17].1),
    ("D18", gpio::LABELS[18].1),
    ("LED", LED_PIN),
];

/// Python modules built into the firmware: micropython-esp32's `machine`
/// and `time`, on the same functions.
pub static FROZEN_MODULES: &[(&str, &str)] = &[
    (
        "machine",
        include_str!("../../micropython-esp32/modules/machine.py"),
    ),
    (
        "time",
        include_str!("../../micropython-esp32/modules/time.py"),
    ),
];

/// The largest frozen module's source (in flash): what `machine.bench()`'s
/// flash read test reads.
pub fn frozen_text() -> &'static str {
    FROZEN_MODULES
        .iter()
        .map(|&(_, text)| text)
        .max_by_key(|text| text.len())
        .unwrap_or("")
}

pub struct Board {
    /// Why the chip started (`reset_cause()`): read from EMU at boot.
    reset_cause: i64,
    i2c: Option<I2c>,
}

impl Board {
    pub fn new() -> Self {
        // SAFETY: EMU's reset cause register, read once and cleared at boot
        let reset_cause = unsafe {
            let emu = &*pac::EmuS::ptr();
            let r = emu.rstcause().read();
            let cause =
                if r.por().bit_is_set() || r.dvddbod().bit_is_set() || r.avddbod().bit_is_set() {
                    PWRON_RESET
                } else if r.wdog0().bit_is_set() || r.wdog1().bit_is_set() {
                    WDT_RESET
                } else if r.em4().bit_is_set() {
                    DEEPSLEEP_RESET
                } else if r.sysreq().bit_is_set() {
                    // machine.reset(), a panic, the debugger (probe-rs reset)
                    SOFT_RESET
                } else {
                    HARD_RESET // the reset pin, a lockup
                };
            emu.cmd().write(|w| w.rstcauseclr().set_bit());
            cause
        };
        Self {
            reset_cause,
            i2c: None,
        }
    }

    /// Busy-waits `ms`; Ctrl-C on the console stops it.
    fn sleep_ms(&mut self, ms: i64) -> Result<(), Error> {
        let us = non_negative(ms)?.saturating_mul(1000) as u64;
        let until = clock::now_us().saturating_add(us);
        while clock::now_us() < until {
            if micropython_port::ABORT.load(Ordering::Relaxed) {
                return Err(Error::Interrupted);
            }
        }
        Ok(())
    }

    /// The I2C bus, on the default pins at 100 kHz unless `i2c_init` chose.
    fn i2c(&mut self) -> Result<&mut I2c, Error> {
        if self.i2c.is_none() {
            let (sda, scl) = (PinId::new(gpio::I2C_SDA)?, PinId::new(gpio::I2C_SCL)?);
            self.i2c = Some(I2c::init(sda, scl, I2C_DEFAULT_HZ, pclk_hz())?);
        }
        Ok(self.i2c.as_mut().unwrap())
    }
}

fn pclk_hz() -> u32 {
    // PCLK is the CPU clock (no prescaler below 50 MHz)
    mg24_hal::clock::sysclk_hz()
}

fn non_negative(v: i64) -> Result<i64, Error> {
    if v < 0 {
        Err(Error::ValueError)
    } else {
        Ok(v)
    }
}

fn i2c_addr(v: i64) -> Result<u8, Error> {
    u8::try_from(v)
        .ok()
        .filter(|a| *a < 0x80)
        .ok_or(Error::ValueError)
}

fn byte(v: i64) -> Result<u8, Error> {
    u8::try_from(v).map_err(|_| Error::ValueError)
}

fn pin(v: i64) -> Result<PinId, Error> {
    PinId::new(v)
}

/// The EUI-64 from the chip's DEVINFO, most significant byte first.
fn eui64() -> [u8; 8] {
    // SAFETY: DEVINFO is read-only factory data
    let devinfo = unsafe { &*pac::Devinfo::ptr() };
    let (h, l) = (
        devinfo.eui64h().read().bits(),
        devinfo.eui64l().read().bits(),
    );
    let mut id = [0u8; 8];
    id[..4].copy_from_slice(&h.to_be_bytes());
    id[4..].copy_from_slice(&l.to_be_bytes());
    id
}

impl HardwareBridge for Board {
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        let int = |i: usize| args[i].int();
        match name {
            "sleep_ms" | "delay_ms" => self.sleep_ms(int(0)?).map(|()| None),
            "sleep_us" => {
                clock::delay_us(non_negative(int(0)?)? as u64);
                Ok(None)
            }
            "ticks_ms" => Ok(Some(clock::now_ms() as i64)),
            "ticks_us" => Ok(Some(clock::now_us() as i64)),
            "ticks_diff" => Ok(Some(int(0)?.wrapping_sub(int(1)?))),
            "pin_mode" => gpio::pin_mode(int(0)?, int(1)?).map(|()| None),
            "pin_write" => {
                gpio::write(pin(int(0)?)?, int(1)? != 0);
                Ok(None)
            }
            "pin_read" => Ok(Some(gpio::read(pin(int(0)?)?) as i64)),
            "pin_toggle" => {
                gpio::toggle(pin(int(0)?)?);
                Ok(None)
            }
            "led_on" | "led_off" => {
                let led = pin(LED_PIN)?;
                gpio::pin_mode(LED_PIN, gpio::OUT)?;
                gpio::write(
                    led,
                    if name == "led_on" {
                        LED_ON_LEVEL
                    } else {
                        !LED_ON_LEVEL
                    },
                );
                Ok(None)
            }
            "i2c_init" => {
                let (sda, scl) = (pin(int(0)?)?, pin(int(1)?)?);
                let hz = match args.get(2) {
                    Some(a) => u32::try_from(a.int()?).map_err(|_| Error::ValueError)?,
                    None => I2C_DEFAULT_HZ,
                };
                self.i2c = Some(I2c::init(sda, scl, hz, pclk_hz())?);
                Ok(None)
            }
            "i2c_probe" => {
                let addr = i2c_addr(int(0)?)?;
                Ok(Some(self.i2c()?.probe(addr) as i64))
            }
            "i2c_write" => {
                let (addr, data) = (i2c_addr(int(0)?)?, args[1].bytes()?);
                self.i2c()?.write(addr, data)?;
                Ok(Some(data.len() as i64))
            }
            "i2c_read_reg" => {
                let (addr, reg) = (i2c_addr(int(0)?)?, byte(int(1)?)?);
                let mut b = [0u8];
                self.i2c()?.write_read(addr, &[reg], &mut b)?;
                Ok(Some(b[0] as i64))
            }
            "i2c_read_reg16" => {
                let (addr, reg) = (i2c_addr(int(0)?)?, byte(int(1)?)?);
                let mut b = [0u8; 2];
                self.i2c()?.write_read(addr, &[reg], &mut b)?;
                Ok(Some(u16::from_be_bytes(b) as i64))
            }
            "i2c_write_reg" => {
                let (addr, reg, value) = (i2c_addr(int(0)?)?, byte(int(1)?)?, byte(int(2)?)?);
                self.i2c()?.write(addr, &[reg, value])?;
                Ok(None)
            }
            "reset" => {
                crate::console::flush();
                // SCB.AIRCR: VECTKEY and SYSRESETREQ
                // SAFETY: a write to the System Control Block's reset register
                unsafe { core::ptr::write_volatile(0xE000_ED0C as *mut u32, 0x05FA_0004) };
                loop {
                    core::hint::spin_loop();
                }
            }
            "reset_cause" => Ok(Some(self.reset_cause)),
            "cpu_freq" => Ok(Some(mg24_hal::clock::sysclk_hz() as i64)),
            "rtc_set" => {
                let us = match args.get(1) {
                    Some(us) => us.int()?,
                    None => 0,
                };
                let wall = int(0)?
                    .checked_mul(1_000_000)
                    .and_then(|s| s.checked_add(us))
                    .ok_or(Error::ValueError)?;
                clock::set_wall_us(wall);
                Ok(None)
            }
            "cpu_bench" => {
                let test = args[0].str()?;
                if let Some(rest) = test.strip_prefix("gpio") {
                    let _ = rest;
                    let n = args.get(1).ok_or(Error::TypeError)?.int()?;
                    let id = pin(n)?;
                    gpio::pin_mode(n, gpio::OUT)?;
                    let r = crate::bench::gpio(id, test).ok_or(Error::ValueError);
                    gpio::write(id, false); // it ends low
                    return r.map(Some);
                }
                crate::bench::run(test).map(Some).ok_or(Error::ValueError)
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }

    fn call_bytes(&mut self, name: &str, args: &[Arg<'_>], out: &mut [u8]) -> Result<usize, Error> {
        let int = |i: usize| args[i].int();
        match name {
            "unique_id" => {
                out[..8].copy_from_slice(&eui64());
                Ok(8)
            }
            "i2c_scan" => {
                let bus = self.i2c()?;
                let mut n = 0;
                for addr in 0x08..0x78 {
                    if bus.probe(addr) {
                        out[n] = addr;
                        n += 1;
                    }
                }
                Ok(n)
            }
            "i2c_read" => {
                let addr = i2c_addr(int(0)?)?;
                let n = (non_negative(int(1)?)? as usize).min(out.len());
                self.i2c()?.read(addr, &mut out[..n])?;
                Ok(n)
            }
            "i2c_write_read" => {
                let (addr, data) = (i2c_addr(int(0)?)?, args[1].bytes()?);
                let n = (non_negative(int(2)?)? as usize).min(out.len());
                self.i2c()?.write_read(addr, data, &mut out[..n])?;
                Ok(n)
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }

    fn call_values(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[Arg<'_>],
        out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        match (module, name) {
            (None, "cpu_info") => cpu_info(self.reset_cause, out),
            (Some("utime"), "gmtime" | "localtime") => {
                let secs = match args.first() {
                    Some(arg) => arg.int()?,
                    None => clock::wall_us().div_euclid(1_000_000),
                };
                let t = micropython_rs::calendar::gmtime(secs);
                out.begin_tuple()?;
                for v in [
                    t.year, t.month, t.mday, t.hour, t.minute, t.second, t.weekday, t.yearday,
                ] {
                    out.int(v)?;
                }
                out.end()
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }

    fn call_module(
        &mut self,
        _module: &str,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        let int = |i: usize| args[i].int();
        match name {
            "sleep" => {
                let s = args[0].float()?;
                if !(0.0..=4.0e6).contains(&s) {
                    return Err(Error::ValueError);
                }
                self.sleep_ms((s * 1000.0) as i64).map(|()| None)
            }
            "ticks_add" => Ok(Some(int(0)?.wrapping_add(int(1)?))),
            "time" => Ok(Some(clock::wall_us().div_euclid(1_000_000))),
            "time_ns" => Ok(Some(clock::wall_us().saturating_mul(1000))),
            "_mktime" => {
                let v = |i: usize| args[i].int();
                Ok(Some(micropython_rs::calendar::mktime(
                    v(0)?,
                    v(1)?,
                    v(2)?,
                    v(3)?,
                    v(4)?,
                    v(5)?,
                )))
            }
            // sleep_ms, sleep_us, ticks_ms, ticks_us, ticks_diff
            _ => self.call_args(name, args),
        }
    }

    fn functions(&self) -> &[HwFn] {
        FUNCTIONS
    }

    fn modules(&self) -> &[Module] {
        &[TIME_MODULE]
    }

    fn constants(&self) -> &[(&'static str, i64)] {
        CONSTANTS
    }
}

/// `cpu_info()`: what `machine.lscpu()` shows, with the ESP32 boards' keys
/// (sizes in bytes); `machine.py` fills in the Python heap.
fn cpu_info(reset_cause: i64, out: &mut Returns<'_>) -> Result<(), Error> {
    use core::fmt::Write;
    // SAFETY: DEVINFO and SYSCFG's chip revision are read-only; SYSCFG's
    // bus clock must be on (an access without it is a bus fault)
    let (devinfo, syscfg) = unsafe {
        (&*pac::CmuS::ptr())
            .clken0()
            .modify(|_, w| w.syscfg().set_bit());
        (&*pac::Devinfo::ptr(), &*pac::SyscfgS::ptr())
    };
    let msize = devinfo.msize().read();
    let rev = syscfg.chiprev().read();
    let mut revision = Text::<8>::new();
    let _ = write!(revision, "v{}.{}", rev.major().bits(), rev.minor().bits());
    let id = eui64();
    let mut eui = Text::<24>::new();
    for (i, b) in id.iter().enumerate() {
        let _ = write!(eui, "{}{:02x}", if i == 0 { "" } else { ":" }, b);
    }
    let mut build = Text::<32>::new();
    let level = if cfg!(feature = "minimal") {
        "minimal"
    } else if cfg!(feature = "medium") {
        "medium"
    } else {
        "full"
    };
    let _ = write!(
        build,
        "{}{}",
        level,
        if cfg!(feature = "float32") {
            " float32"
        } else {
            ""
        }
    );
    let reset = match reset_cause {
        PWRON_RESET => "power on",
        WDT_RESET => "watchdog",
        DEEPSLEEP_RESET => "deep sleep",
        SOFT_RESET => "soft reset",
        _ => "hard reset",
    };
    let text = |out: &mut Returns<'_>, key: &str, value: &str| -> Result<(), Error> {
        out.str(key)?;
        out.str(value)
    };
    let int = |out: &mut Returns<'_>, key: &str, value: i64| -> Result<(), Error> {
        out.str(key)?;
        out.int(value)
    };
    out.begin_dict()?;
    text(out, "Architecture", "armv8-m.main")?;
    text(out, "Byte Order", "Little Endian")?;
    int(out, "CPU(s)", 1)?;
    text(out, "Vendor ID", "Silicon Labs")?;
    text(out, "Model name", "EFR32MG24B220F1536IM48")?;
    text(out, "CPU core", "Arm Cortex-M33")?;
    text(out, "Revision", revision.as_str())?;
    int(
        out,
        "CPU MHz",
        (mg24_hal::clock::sysclk_hz() / 1_000_000) as i64,
    )?;
    int(out, "CPU max MHz", 78)?;
    text(out, "Flags", "fpu dsp trustzone ble 802.15.4")?;
    int(out, "Flash size", msize.flash().bits() as i64 * 1024)?;
    int(out, "SRAM size", msize.sram().bits() as i64 * 1024)?;
    int(out, "PSRAM size", 0)?;
    int(out, "Stack size", crate::stack_size() as i64)?;
    for key in ["Python heap size", "Python heap free"] {
        out.str(key)?;
        out.none()?;
    }
    text(out, "MAC address", eui.as_str())?;
    text(out, "Board", "Seeed XIAO MG24 (Sense)")?;
    text(out, "Firmware", crate::VERSION)?;
    text(out, "Commit", crate::COMMIT)?;
    text(out, "Build", build.as_str())?;
    int(out, "Uptime (ms)", clock::now_ms() as i64)?;
    text(out, "Reset cause", reset)?;
    int(out, "Console bytes lost", crate::console::dropped() as i64)?;
    out.end()
}

/// A small text buffer on the stack, for `write!`.
struct Text<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Text<N> {
    fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> core::fmt::Write for Text<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        self.buf
            .get_mut(self.len..end)
            .ok_or(core::fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
