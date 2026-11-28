//! The XIAO MG24's functions for scripts (`HardwareBridge`): the same names,
//! arguments and constants as micropython-esp32's (pin_mode, pin_write,
//! i2c_read, sleep_ms, cpu_info, ...), so the frozen `machine` and `time`
//! modules from micropython-esp32/modules run unchanged and scripts move
//! between the boards.

use core::sync::atomic::Ordering;

use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Module, Returns};

use crate::adc::Adc;
use crate::clock;
use crate::events::{self, Events};
use crate::gpio::{self, PinId};
use crate::i2c::I2c;
use crate::pwm::Pwm;
use crate::spi::Spi;
use crate::uart::Uart;
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
    HwFn::new(
        "adc_read",
        "pin",
        "Raw 12-bit ADC value, 0-4095 for 0 V to AVDD (any free pin)",
    ),
    HwFn::with_args(
        "adc_mv",
        1,
        2,
        "pin, [samples]",
        "Calibrated millivolts, averaged (default 16 samples)",
    ),
    HwFn::new(
        "adc_vref",
        "",
        "The ADC's reference, AVDD, measured against the 1.21 V bandgap, mV",
    ),
    HwFn::new(
        "pwm",
        "pin, freq, duty",
        "PWM output, duty in percent (0-100)",
    ),
    HwFn::new(
        "pwm_u16",
        "pin, freq, duty",
        "PWM output, duty 0-65535 (12 pins, 4 frequencies at once)",
    ),
    HwFn::new("pwm_stop", "pin", "Stop PWM on a pin"),
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
    HwFn::new(
        "pin_irq",
        "pin, trigger",
        "Pin interrupt events: 1 rising, 2 falling, 3 both, 0 off (use Pin.irq)",
    ),
    HwFn::new(
        "timer_init",
        "id, period_us, periodic",
        "Timer events (use machine.Timer)",
    ),
    HwFn::new("timer_deinit", "id", "Stop a timer"),
    HwFn::new(
        "wdt_enable",
        "timeout_ms",
        "Reset the board unless fed in time (up to 262 s)",
    ),
    HwFn::new("wdt_feed", "", "Feed the watchdog"),
    HwFn::new("wdt_disable", "", "Turn the watchdog off"),
    HwFn::with_args("uart_init", 2, 3, "tx, rx, [baud]", "UART on these pins (EUSART1, any port; default 115200 baud)"),
    HwFn::new("uart_write", "data", "Send bytes or a str; returns how many"),
    HwFn::with_args("uart_read", 0, 2, "[n], [timeout_ms]", "Up to n received bytes (default 64), waiting up to timeout_ms").returning_bytes(),
    HwFn::new("uart_any", "", "Number of received bytes waiting"),
    HwFn::with_args("spi_init", 3, 5, "sck, mosi, miso, [khz], [mode]", "SPI main (EUSART0, ports A/B: D8 SCK, D10 MOSI, D9 MISO; default 1000 kHz, mode 0); CS: any pin_write"),
    HwFn::new("spi_write", "data", "Send bytes"),
    HwFn::new("spi_transfer", "data", "Send bytes, return the bytes received meanwhile").returning_bytes(),
    HwFn::with_args("spi_read", 1, 2, "n, [fill]", "Receive n bytes (sending fill, default 0)").returning_bytes(),
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
    /// Set up on first use (its start measures AVDD)
    adc: Option<Adc>,
    pwm: Pwm,
    uart: Option<Uart>,
    spi: Option<Spi>,
    events: Events,
    /// A script enabled the watchdog: sleeps feed it (as on the ESP32 boards)
    wdt_on: bool,
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
            adc: None,
            pwm: Pwm::new(mg24_hal::clock::sysclk_hz()),
            uart: None,
            spi: None,
            events: Events::new(),
            wdt_on: false,
        }
    }

    fn sleep_ms(&mut self, ms: i64) -> Result<(), Error> {
        self.sleep_for_us(non_negative(ms)?.saturating_mul(1000) as u64)
    }

    /// Busy-waits `us`: Ctrl-C on the console stops it, an event that's due
    /// (a pin interrupt, a timer) stops it with `Error::Pending` (the
    /// interpreter runs the callback and resumes the sleep), and it feeds
    /// the watchdog, so a long sleep doesn't trip it.
    fn sleep_for_us(&mut self, us: u64) -> Result<(), Error> {
        let until = clock::now_us().saturating_add(us);
        loop {
            if micropython_port::ABORT.load(Ordering::Relaxed) {
                return Err(Error::Interrupted);
            }
            let now = clock::now_us();
            if now >= until {
                return Ok(());
            }
            if self.events.on && self.events.waiting(now) {
                let left_ms = (until - now).div_ceil(1000);
                return Err(Error::Pending(left_ms.min(u32::MAX as u64) as u32));
            }
            if self.wdt_on {
                watchdog::feed();
            }
        }
    }

    fn adc(&mut self) -> &mut Adc {
        self.adc.get_or_insert_with(Adc::new)
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
            "adc_read" => {
                let id = pin(int(0)?)?;
                Ok(Some(self.adc().read(id)? as i64))
            }
            "adc_mv" => {
                let id = pin(int(0)?)?;
                let samples = match args.get(1) {
                    Some(n) => u32::try_from(n.int()?)
                        .ok()
                        .filter(|n| (1..=1024).contains(n))
                        .ok_or(Error::ValueError)?,
                    None => 16,
                };
                Ok(Some(self.adc().read_mv(id, samples)? as i64))
            }
            "adc_vref" => Ok(Some(self.adc().avdd_mv() as i64)),
            "pwm" | "pwm_u16" => {
                let id = pin(int(0)?)?;
                let freq = u32::try_from(int(1)?).map_err(|_| Error::ValueError)?;
                let duty = int(2)?;
                let duty_u16 = if name == "pwm" {
                    if !(0..=100).contains(&duty) {
                        return Err(Error::ValueError);
                    }
                    duty * 65535 / 100
                } else {
                    duty
                };
                let duty_u16 = u32::try_from(duty_u16).map_err(|_| Error::ValueError)?;
                self.pwm.set(id, freq, duty_u16).map(|()| None)
            }
            "pwm_stop" => {
                self.pwm.stop(pin(int(0)?)?);
                Ok(None)
            }
            "uart_init" => {
                let (tx, rx) = (pin(int(0)?)?, pin(int(1)?)?);
                let baud = match args.get(2) {
                    Some(b) => u32::try_from(b.int()?).map_err(|_| Error::ValueError)?,
                    None => 115_200,
                };
                self.uart = Some(Uart::init(tx, rx, baud, mg24_hal::clock::sysclk_hz())?);
                Ok(None)
            }
            "uart_write" => {
                let data = args[0].bytes()?;
                self.uart.as_mut().ok_or(Error::IoError)?.write(data);
                Ok(Some(data.len() as i64))
            }
            "uart_any" => Ok(Some(self.uart.as_ref().ok_or(Error::IoError)?.any() as i64)),
            "spi_init" => {
                let (sck, mosi, miso) = (pin(int(0)?)?, pin(int(1)?)?, pin(int(2)?)?);
                let khz = match args.get(3) {
                    Some(k) => u32::try_from(k.int()?).map_err(|_| Error::ValueError)?,
                    None => 1000,
                };
                let mode = match args.get(4) {
                    Some(m) => u8::try_from(m.int()?).map_err(|_| Error::ValueError)?,
                    None => 0,
                };
                self.spi = Some(Spi::init(
                    sck,
                    mosi,
                    miso,
                    khz,
                    mode,
                    mg24_hal::clock::sysclk_hz(),
                )?);
                Ok(None)
            }
            "spi_write" => {
                let spi = self.spi.as_mut().ok_or(Error::IoError)?;
                let mut chunk = [0u8; 64];
                for part in args[0].bytes()?.chunks(chunk.len()) {
                    chunk[..part.len()].copy_from_slice(part);
                    spi.transfer(&mut chunk[..part.len()])?;
                }
                Ok(None)
            }
            "pin_irq" => {
                let id = pin(int(0)?)?;
                self.events.pin_irq(id, int(1)?).map(|()| None)
            }
            "timer_init" => {
                let slot = events::timer_slot(int(0)?)?;
                let period = u64::try_from(int(1)?).map_err(|_| Error::ValueError)?;
                if period == 0 {
                    return Err(Error::ValueError);
                }
                self.events
                    .start_timer(slot, period, int(2)? != 0, clock::now_us());
                Ok(None)
            }
            "timer_deinit" => {
                self.events.stop_timer(events::timer_slot(int(0)?)?);
                Ok(None)
            }
            "wdt_enable" => {
                watchdog::enable(non_negative(int(0)?)?)?;
                self.wdt_on = true;
                Ok(None)
            }
            "wdt_feed" => {
                watchdog::feed();
                Ok(None)
            }
            "wdt_disable" => {
                watchdog::disable();
                self.wdt_on = false;
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
            "uart_read" => {
                let n = match args.first() {
                    Some(n) => (non_negative(n.int()?)? as usize).min(out.len()),
                    None => 64,
                };
                let timeout = match args.get(1) {
                    Some(t) => non_negative(t.int()?)? as u64,
                    None => 0,
                };
                let until = clock::now_us() + timeout * 1000;
                let uart = self.uart.as_mut().ok_or(Error::IoError)?;
                let mut got = 0;
                loop {
                    got += uart.read(&mut out[got..n]);
                    if got == n || clock::now_us() >= until {
                        return Ok(got);
                    }
                    if micropython_port::ABORT.load(Ordering::Relaxed) {
                        return Err(Error::Interrupted);
                    }
                }
            }
            "spi_transfer" => {
                let data = args[0].bytes()?;
                let n = data.len().min(out.len());
                out[..n].copy_from_slice(&data[..n]);
                self.spi
                    .as_mut()
                    .ok_or(Error::IoError)?
                    .transfer(&mut out[..n])?;
                Ok(n)
            }
            "spi_read" => {
                let n = (non_negative(int(0)?)? as usize).min(out.len());
                let fill = match args.get(1) {
                    Some(f) => byte(f.int()?)?,
                    None => 0,
                };
                out[..n].fill(fill);
                self.spi
                    .as_mut()
                    .ok_or(Error::IoError)?
                    .transfer(&mut out[..n])?;
                Ok(n)
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

    fn poll_event(&mut self) -> Option<i64> {
        self.events.poll(clock::now_us())
    }

    fn set_events(&mut self, on: bool) {
        self.events.on = on;
    }

    fn resume_sleep(&mut self, remaining_ms: u32) -> Result<(), Error> {
        self.sleep_for_us(remaining_ms as u64 * 1000)
    }

    /// A soft reset: no more pin interrupts and timers from the last session.
    fn soft_reset(&mut self) {
        self.events.reset();
        self.events.on = false;
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

/// WDOG0: resets the chip unless fed in time. Its clock is the 32 kHz RC
/// oscillator for timeouts up to 8 s, the 1 kHz one beyond (up to 262 s);
/// the period is 2^(n + 3) of its cycles, the shortest that covers the
/// timeout (so it may be up to twice as long, as MicroPython allows).
mod watchdog {
    use efr32mg24_pac as pac;
    use micropython_rs::Error;

    fn wdog() -> &'static pac::wdog0_s::RegisterBlock {
        // SAFETY: WDOG0's registers; only this module uses them
        unsafe { &*pac::Wdog0S::ptr() }
    }

    fn wait_sync() {
        while wdog().syncbusy().read().bits() != 0 {}
    }

    pub fn enable(timeout_ms: i64) -> Result<(), Error> {
        let (clksel, hz) = if timeout_ms <= 8_000 {
            (1u8, 32_768u64)
        } else {
            (3u8, 1_000u64)
        };
        let cycles = (timeout_ms as u64 * hz).div_ceil(1000).max(1);
        // The smallest PERSEL (0-15) with 2^(PERSEL + 3) cycles >= the timeout
        let persel = (0..16u32)
            .find(|n| 1u64 << (n + 3) >= cycles)
            .ok_or(Error::ValueError)?;
        // SAFETY: CMU's clock enable and WDOG0's clock select, then WDOG0;
        // CFG can only be written while it's disabled
        unsafe {
            let cmu = &*pac::CmuS::ptr();
            cmu.clken0().modify(|_, w| w.wdog0().set_bit());
            disable();
            cmu.wdog0clkctrl().write(|w| w.bits(clksel as u32));
            wdog().cfg().write(|w| w.persel().bits(persel as u8));
            wdog().en().write(|w| w.en().set_bit());
        }
        feed();
        Ok(())
    }

    pub fn feed() {
        wait_sync();
        wdog().cmd().write(|w| w.clear().set_bit());
    }

    pub fn disable() {
        // SAFETY: CMU's WDOG0 bus clock (it may not be on yet)
        unsafe {
            (&*pac::CmuS::ptr())
                .clken0()
                .modify(|_, w| w.wdog0().set_bit())
        };
        wait_sync();
        wdog().en().write(|w| w.en().clear_bit());
        // Disabling takes a few of its clock cycles (EN's DISABLING bit);
        // CFG can't be written before it's done (a bus fault)
        while wdog().en().read().bits() & 0b10 != 0 {}
    }
}
