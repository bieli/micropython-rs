//! Hardware functions shared by every ESP32 board build:
//! time, GPIO, ADC, PWM, I2C, UART, SPI and the watchdog.
//!
//! Scripts address pins by number at runtime (`pin_write(2, 1)`), so pins
//! are taken with `AnyPin::steal` and driven through `Flex`. Only pins in the
//! board's allowlist can be used, which keeps scripts away from the flash
//! pins (GPIO6-11), the USB serial console (GPIO1/3) and pins that the
//! firmware already uses (display, I2C, ADC).

use esp_hal::{
    analog::adc::{Adc, AdcConfig, AdcPin, Attenuation},
    delay::Delay,
    gpio::{AnyPin, DriveMode, Flex, InputConfig, Level, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    ledc::{
        channel::{self, ChannelHW, ChannelIFace},
        timer::{self, TimerIFace},
        LSGlobalClkSource, Ledc, LowSpeed,
    },
    peripherals::{ADC1, I2C0, LEDC, LPWR},
    rtc_cntl::{Rtc, RwdtStage, RwdtStageAction, SocResetReason},
    spi::{
        master::{Config as SpiConfig, Spi},
        Mode as SpiMode,
    },
    time::{Duration, Instant, Rate},
    uart::{Config as UartConfig, Uart},
    Blocking,
};
use micropython_rs::{Arg, Error, HwFn, Module};

use crate::chip;

/// Pin modes for `pin_mode(pin, mode)`; also exported as script constants.
pub const OUT: i64 = 1;
pub const IN: i64 = 2;
pub const IN_PULLUP: i64 = 3;
pub const IN_PULLDOWN: i64 = 4;
/// Open-drain output (drives low, floats high), and the same with the pull-up.
pub const OUT_OD: i64 = 5;
pub const OUT_OD_PULLUP: i64 = 6;

/// The CPU clock, chosen at build time (`make flash CPU_MHZ=240`; see
/// build.rs). esp-hal sets it once, in `esp_hal::init`.
#[cfg(cpu_mhz = "80")]
pub const CPU_CLOCK: esp_hal::clock::CpuClock = esp_hal::clock::CpuClock::_80MHz;
#[cfg(cpu_mhz = "160")]
pub const CPU_CLOCK: esp_hal::clock::CpuClock = esp_hal::clock::CpuClock::_160MHz;
#[cfg(cpu_mhz = "240")]
pub const CPU_CLOCK: esp_hal::clock::CpuClock = esp_hal::clock::CpuClock::_240MHz;

/// `reset_cause()` results, with MicroPython's `machine` module values.
pub const PWRON_RESET: i64 = 1;
pub const HARD_RESET: i64 = 2;
pub const WDT_RESET: i64 = 3;
pub const DEEPSLEEP_RESET: i64 = 4;
pub const SOFT_RESET: i64 = 5;

/// `import time`: MicroPython's `time` module names for the time functions.
pub const TIME_MODULE: Module = Module {
    // With the `machine` feature, the frozen `time` module (modules/time.py)
    // adds the calendar functions (gmtime, mktime) to this one, as MicroPython's
    // `utime`; without it, this is `time`
    #[cfg(feature = "machine")]
    name: "utime",
    #[cfg(not(feature = "machine"))]
    name: "time",
    doc: "Delays, timestamps and the clock (MicroPython names)",
    functions: &[
        HwFn::new("sleep", "s", "Wait s seconds (a float: 0.5)"),
        HwFn::new("sleep_ms", "ms", "Wait ms milliseconds"),
        HwFn::new("sleep_us", "us", "Wait us microseconds"),
        HwFn::new("ticks_ms", "", "Milliseconds since boot"),
        HwFn::new("ticks_us", "", "Microseconds since boot"),
        HwFn::new("ticks_diff", "new, old", "new - old"),
        HwFn::new("ticks_add", "ticks, delta", "ticks + delta"),
        HwFn::new(
            "time",
            "",
            "Seconds since 2000-01-01 (UTC, MicroPython's ESP32 epoch), from the clock",
        ),
        HwFn::new("time_ns", "", "Nanoseconds since 2000-01-01 (UTC)"),
    ],
    constants: &[],
};

/// The clock at boot: 2000-01-01 00:00:00 UTC, as MicroPython's RTC starts,
/// until `rtc_set` (or `ntptime.settime()`). The clock counts microseconds
/// since then: MicroPython's ESP32 epoch is 2000, not 1970.
const CLOCK_AT_BOOT_US: i64 = 0;

/// Pins wired to the ADC (ESP32: the input-only GPIO34/36/39, so analog
/// inputs never collide with digital outputs; ESP32-S3: GPIO1-3), and the
/// default I2C bus pins: see `chip`.
pub use crate::chip::{ADC_PINS, I2C_SCL, I2C_SDA};

/// Number of PWM outputs that can run at the same time (one LEDC timer each,
/// so every output can have its own frequency).
const PWM_SLOTS: usize = 4;
/// Longest busy-wait between watchdog feeds while sleeping.
const SLEEP_CHUNK_MS: i64 = 100;

/// Hardware functions every board provides, plus `extra` board-specific ones.
#[macro_export]
macro_rules! board_functions {
    ($($extra:expr),* $(,)?) => {
        &[
            micropython_rs::HwFn::new("sleep_ms", "ms", "Wait ms milliseconds (feeds the watchdog)"),
            micropython_rs::HwFn::new("sleep_us", "us", "Wait us microseconds"),
            micropython_rs::HwFn::new("delay_ms", "ms", "Same as sleep_ms"),
            micropython_rs::HwFn::new("ticks_ms", "", "Milliseconds since boot"),
            micropython_rs::HwFn::new("ticks_us", "", "Microseconds since boot"),
            micropython_rs::HwFn::new("ticks_diff", "new, old", "new - old"),
            micropython_rs::HwFn::new("pin_mode", "pin, mode", "mode: OUT, IN, IN_PULLUP, IN_PULLDOWN, OUT_OD, OUT_OD_PULLUP"),
            micropython_rs::HwFn::new("pin_write", "pin, value", "Set an output pin to 0 or 1"),
            micropython_rs::HwFn::new("pin_read", "pin", "Read a pin: 0 or 1"),
            micropython_rs::HwFn::new("pin_toggle", "pin", "Invert an output pin"),
            micropython_rs::HwFn::new("adc_read", "pin", "Raw 12-bit ADC value (ESP32: pins 34, 36, 39; ESP32-S3: 1, 2, 3)"),
            micropython_rs::HwFn::with_args("adc_mv", 1, 2, "pin, [samples]", "Calibrated millivolts, averaged (default 16 samples)"),
            micropython_rs::HwFn::new("adc_vref", "", "ADC reference voltage from eFuse, mV"),
            micropython_rs::HwFn::new("pwm", "pin, freq, duty", "PWM output, duty in percent (0-100)"),
            micropython_rs::HwFn::new("pwm_u16", "pin, freq, duty", "PWM output, duty 0-65535"),
            micropython_rs::HwFn::new("pwm_stop", "pin", "Stop PWM on a pin"),
            micropython_rs::HwFn::with_args("i2c_init", 2, 3, "sda, scl, [freq]", "Move the I2C bus (default 21, 22 at 100 kHz)"),
            micropython_rs::HwFn::new("i2c_probe", "addr", "True if a device answers at addr"),
            micropython_rs::HwFn::new("i2c_scan", "", "Addresses of the devices on the bus (bytes)").returning_bytes(),
            micropython_rs::HwFn::new("cpu_info", "", "Chip, CPU, memory and build details as key=value lines (use machine.lscpu())").returning_bytes(),
            micropython_rs::HwFn::new("i2c_write", "addr, data", "Send bytes; returns how many"),
            micropython_rs::HwFn::new("i2c_read", "addr, n", "Receive n bytes").returning_bytes(),
            micropython_rs::HwFn::new("i2c_write_read", "addr, data, n", "Send, then receive n bytes (e.g. a register)").returning_bytes(),
            micropython_rs::HwFn::new("i2c_read_reg", "addr, reg", "Read one register byte"),
            micropython_rs::HwFn::new("i2c_read_reg16", "addr, reg", "Read a big-endian 16-bit register"),
            micropython_rs::HwFn::new("i2c_write_reg", "addr, reg, value", "Write one register byte"),
            micropython_rs::HwFn::with_args("uart_init", 2, 3, "tx, rx, [baud]", "UART1 on these pins (default 115200 baud)"),
            micropython_rs::HwFn::new("uart_write", "data", "Send bytes or a str; returns how many"),
            micropython_rs::HwFn::with_args("uart_read", 0, 2, "[n], [timeout_ms]", "Up to n received bytes (default 64), waiting up to timeout_ms").returning_bytes(),
            micropython_rs::HwFn::new("uart_any", "", "Number of received bytes waiting"),
            micropython_rs::HwFn::with_args("spi_init", 3, 5, "sck, mosi, miso, [khz], [mode]", "SPI3 master (default 1000 kHz, mode 0); CS: any pin_write"),
            micropython_rs::HwFn::new("spi_write", "data", "Send bytes"),
            micropython_rs::HwFn::new("spi_transfer", "data", "Send bytes, return the bytes received meanwhile").returning_bytes(),
            micropython_rs::HwFn::with_args("spi_read", 1, 2, "n, [fill]", "Receive n bytes (sending fill, default 0)").returning_bytes(),
            micropython_rs::HwFn::new("wdt_enable", "timeout_ms", "Reset the board unless fed in time"),
            micropython_rs::HwFn::new("wdt_feed", "", "Feed the watchdog"),
            micropython_rs::HwFn::new("wdt_disable", "", "Turn the watchdog off"),
            micropython_rs::HwFn::new("reset", "", "Restart the board"),
            micropython_rs::HwFn::new("reset_cause", "", "PWRON_RESET, WDT_RESET, SOFT_RESET, ..."),
            micropython_rs::HwFn::new("stack_peak", "", "Most stack bytes used since the last call"),
            micropython_rs::HwFn::new("unique_id", "", "The chip's MAC address (6 bytes)").returning_bytes(),
            micropython_rs::HwFn::new("cpu_freq", "", "CPU clock in Hz"),
            micropython_rs::HwFn::with_args("rtc_set", 1, 2, "seconds, [microseconds]", "Set the clock: seconds since 2000-01-01 (UTC)"),
            micropython_rs::HwFn::new("pin_irq", "pin, trigger", "Pin interrupt events: 1 rising, 2 falling, 3 both, 0 off (use Pin.irq)"),
            micropython_rs::HwFn::new("timer_init", "id, period_us, periodic", "Timer events (use machine.Timer)"),
            micropython_rs::HwFn::new("timer_deinit", "id", "Stop a timer"),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::with_args("wifi_active", 0, 1, "[on]", "Wi-Fi station on/off; returns whether it's on"),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::new("wifi_scan", "", "Access points (encoded; use network.WLAN.scan)").returning_bytes(),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::with_args("wifi_connect", 1, 2, "ssid, [key]", "Start connecting to a network"),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::new("wifi_disconnect", "", "Disconnect"),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::new("wifi_status", "", "network.STAT_IDLE / CONNECTING / GOT_IP / ..."),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::new("wifi_ifconfig", "", "IP, netmask, gateway, DNS (16 bytes)").returning_bytes(),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::new("wifi_mac", "", "The station's MAC address (6 bytes)").returning_bytes(),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::with_args("wifi_ap_active", 0, 1, "[on]", "Access point on/off; returns whether it's on"),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::with_args("wifi_ap_config", 6, 6, "ssid, key, authmode, channel, hidden, max_clients", "Configure the access point"),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::new("wifi_ap_ifconfig", "", "The access point's IP, netmask, gateway, DNS (16 bytes)").returning_bytes(),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::new("wifi_ap_stations", "", "MAC addresses of the devices joined (6 bytes each)").returning_bytes(),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::new("wifi_ap_mac", "", "The access point's MAC address (6 bytes)").returning_bytes(),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::new("wifi_rssi", "", "Signal strength of the connection, dBm"),
            #[cfg(feature = "wifi")]
            micropython_rs::HwFn::with_args("wifi_ntp", 1, 2, "host, [timeout_ms]", "Time from an NTP server: microseconds since 2000-01-01 (use ntptime)"),
            $($extra),*
        ]
    };
}

/// Constants every board provides, plus `extra` board-specific ones.
#[macro_export]
macro_rules! board_constants {
    ($($extra:expr),* $(,)?) => {
        &[
            ("OUT", $crate::board::OUT),
            ("IN", $crate::board::IN),
            ("IN_PULLUP", $crate::board::IN_PULLUP),
            ("IN_PULLDOWN", $crate::board::IN_PULLDOWN),
            ("OUT_OD", $crate::board::OUT_OD),
            ("OUT_OD_PULLUP", $crate::board::OUT_OD_PULLUP),
            ("HIGH", 1),
            ("LOW", 0),
            ("PWRON_RESET", $crate::board::PWRON_RESET),
            ("HARD_RESET", $crate::board::HARD_RESET),
            ("WDT_RESET", $crate::board::WDT_RESET),
            ("DEEPSLEEP_RESET", $crate::board::DEEPSLEEP_RESET),
            ("SOFT_RESET", $crate::board::SOFT_RESET),
            // The chip's default I2C pins (machine.I2C() without pins)
            ("I2C_SDA", $crate::board::I2C_SDA as i64),
            ("I2C_SCL", $crate::board::I2C_SCL as i64),
            $($extra),*
        ]
    };
}

/// The peripherals the board module needs.
pub struct BoardPeripherals<'d> {
    /// For the GPIO interrupt handler (`Pin.irq()`)
    pub io_mux: esp_hal::peripherals::IO_MUX<'d>,
    /// The Wi-Fi radio and the timer its scheduler runs on (`wifi` feature)
    #[cfg(feature = "wifi")]
    pub wifi: esp_hal::peripherals::WIFI<'static>,
    #[cfg(feature = "wifi")]
    pub timg1: esp_hal::peripherals::TIMG1<'static>,
    #[cfg(all(feature = "wifi", feature = "esp32c3"))]
    pub sw_int0: esp_hal::interrupt::software::SoftwareInterrupt<'static, 0>,
    pub adc1: ADC1<'d>,
    /// The ADC inputs, `ADC_PINS` (ESP32: GPIO34/36/39, ESP32-S3: GPIO1-3)
    pub adc_a: chip::AdcA<'d>,
    pub adc_b: chip::AdcB<'d>,
    pub adc_c: chip::AdcC<'d>,
    pub ledc: LEDC<'d>,
    pub i2c0: I2C0<'d>,
    pub sda: chip::Sda<'d>,
    pub scl: chip::Scl<'d>,
    pub lpwr: LPWR<'d>,
}

/// ADC1 with its three pins (`ADC_PINS`), at 11 dB attenuation (0-3.1 V).
struct Analog<'d> {
    adc: Adc<'d, ADC1<'d>, Blocking>,
    pin_a: AdcPin<chip::AdcA<'d>, ADC1<'d>, chip::AdcCal<'d>>,
    pin_b: AdcPin<chip::AdcB<'d>, ADC1<'d>, chip::AdcCal<'d>>,
    pin_c: AdcPin<chip::AdcC<'d>, ADC1<'d>, chip::AdcCal<'d>>,
}

impl<'d> Analog<'d> {
    fn new(adc1: ADC1<'d>, a: chip::AdcA<'d>, b: chip::AdcB<'d>, c: chip::AdcC<'d>) -> Self {
        let mut config = AdcConfig::new();
        // ESP32-S3 and C3: calibrated with the chip's eFuse values (`chip::AdcCal`)
        #[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
        let (pin_a, pin_b, pin_c) = (
            config.enable_pin_with_cal(a, Attenuation::_11dB),
            config.enable_pin_with_cal(b, Attenuation::_11dB),
            config.enable_pin_with_cal(c, Attenuation::_11dB),
        );
        #[cfg(feature = "esp32")]
        let (pin_a, pin_b, pin_c) = (
            config.enable_pin(a, Attenuation::_11dB),
            config.enable_pin(b, Attenuation::_11dB),
            config.enable_pin(c, Attenuation::_11dB),
        );
        let adc = Adc::new(adc1, config);
        // esp-hal 1.0 bug workaround: on the ESP32, ADC1 results from the RTC
        // controller come out inverted (4095 - value) unless SAR1_DATA_INV is
        // set, like ESP-IDF does. `Adc::new` only sets the ADC2 bit.
        #[cfg(feature = "esp32")]
        esp_hal::peripherals::SENS::regs()
            .sar_read_ctrl()
            .modify(|_, w| w.sar1_data_inv().set_bit());
        Self {
            adc,
            pin_a,
            pin_b,
            pin_c,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum PinMode {
    Output,
    Input,
}

pub struct Board<'d> {
    delay: Delay,
    /// Pins scripts may use.
    allowed: &'static [u8],
    pins: [Option<(Flex<'d>, PinMode)>; chip::GPIO_COUNT],
    /// ADC1 and its three pins (`None` only while `refresh_adc` rebuilds it)
    analog: Option<Analog<'d>>,
    ledc: Ledc<'d>,
    pwm_pins: [Option<u8>; PWM_SLOTS],
    /// The I2C bus (`None` only while `i2c_init` replaces it).
    i2c: Option<I2c<'d, Blocking>>,
    /// UART1 and SPI3, once a script has set them up (`uart_init`, `spi_init`).
    uart: Option<Uart<'static, Blocking>>,
    spi: Option<Spi<'static, Blocking>>,
    /// Wi-Fi: the peripherals until a script activates it, then the driver
    #[cfg(feature = "wifi")]
    wifi_parts: Option<crate::wifi::WifiParts>,
    #[cfg(feature = "wifi")]
    station: Option<crate::wifi::Wifi>,
    rtc: Rtc<'d>,
    /// `Pin.irq()` and `machine.Timer` events
    events: crate::events::Events,
    /// The clock: microseconds since 2000-01-01 (UTC) minus since boot
    clock_offset_us: i64,
    _io: esp_hal::gpio::Io<'d>,
    wdt_enabled: bool,
}

impl<'d> Board<'d> {
    /// `allowed` lists the GPIO numbers scripts may use with `pin_*` and `pwm`.
    pub fn new(p: BoardPeripherals<'d>, allowed: &'static [u8]) -> Self {
        let analog = Analog::new(p.adc1, p.adc_a, p.adc_b, p.adc_c);

        let mut ledc = Ledc::new(p.ledc);
        ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);

        let i2c = I2c::new(
            p.i2c0,
            I2cConfig::default().with_frequency(Rate::from_khz(100)),
        )
        .unwrap()
        .with_sda(p.sda)
        .with_scl(p.scl);

        Self {
            delay: Delay::new(),
            allowed,
            pins: [const { None }; chip::GPIO_COUNT],
            analog: Some(analog),
            ledc,
            pwm_pins: [None; PWM_SLOTS],
            i2c: Some(i2c),
            uart: None,
            spi: None,
            #[cfg(feature = "wifi")]
            wifi_parts: Some(crate::wifi::WifiParts {
                wifi: p.wifi,
                timg1: p.timg1,
                #[cfg(feature = "esp32c3")]
                sw_int0: p.sw_int0,
            }),
            #[cfg(feature = "wifi")]
            station: None,
            rtc: Rtc::new(p.lpwr),
            events: crate::events::Events::new(),
            clock_offset_us: CLOCK_AT_BOOT_US,
            _io: {
                let mut io = esp_hal::gpio::Io::new(p.io_mux);
                io.set_interrupt_handler(crate::events::gpio_interrupt);
                io
            },
            wdt_enabled: false,
        }
    }

    /// Runs a board function. Returns `UnknownIdentifier` for names that
    /// aren't board functions, so a board build can try its own first.
    pub fn call(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        let int = |i: usize| args[i].int();
        match name {
            "sleep_ms" | "delay_ms" => self.sleep_ms(int(0)?).map(|()| None),
            "sleep_us" => {
                self.delay.delay_micros(non_negative(int(0)?)? as u32);
                Ok(None)
            }
            "ticks_ms" => Ok(Some(since_boot().as_millis() as i64)),
            "ticks_us" => Ok(Some(since_boot().as_micros() as i64)),
            "ticks_diff" => Ok(Some(int(0)?.wrapping_sub(int(1)?))),
            "pin_mode" => self.pin_mode(pin_number(int(0)?)?, int(1)?).map(|()| None),
            "pin_write" => {
                let level = Level::from(int(1)? != 0);
                self.output(pin_number(int(0)?)?)?.set_level(level);
                Ok(None)
            }
            "pin_toggle" => {
                self.output(pin_number(int(0)?)?)?.toggle();
                Ok(None)
            }
            "pin_read" => Ok(Some(self.input(pin_number(int(0)?)?)?.is_high() as i64)),
            "adc_read" => self.adc_read(int(0)?).map(Some),
            "adc_mv" => {
                let samples = match args.get(1) {
                    Some(arg) => arg.int()?,
                    None => DEFAULT_ADC_SAMPLES,
                };
                self.adc_mv(int(0)?, samples).map(Some)
            }
            "adc_vref" => Ok(Some(adc_vref_mv() as i64)),
            "pwm" => {
                // Percent, as a 16-bit fraction (100 % = 65535)
                let pct = int(2)?;
                if !(0..=100).contains(&pct) {
                    return Err(Error::ValueError);
                }
                self.pwm(pin_number(int(0)?)?, int(1)?, (pct * 65535 + 50) / 100)
                    .map(|()| None)
            }
            "pwm_u16" => self
                .pwm(pin_number(int(0)?)?, int(1)?, int(2)?)
                .map(|()| None),
            "i2c_init" => {
                let freq = match args.get(2) {
                    Some(f) => f.int()?,
                    None => 100_000,
                };
                self.i2c_init(pin_number(int(0)?)?, pin_number(int(1)?)?, freq)
                    .map(|()| None)
            }
            "cpu_freq" => Ok(Some(CPU_CLOCK as i64 * 1_000_000)),
            "rtc_set" => {
                let us = match args.get(1) {
                    Some(us) => us.int()?,
                    None => 0,
                };
                let wall_us = int(0)?
                    .checked_mul(1_000_000)
                    .and_then(|s| s.checked_add(us));
                let wall_us = wall_us.ok_or(Error::ValueError)?;
                self.clock_offset_us = wall_us - since_boot().as_micros() as i64;
                Ok(None)
            }
            "pin_irq" => {
                let pin = pin_number(int(0)?)?;
                let trigger = int(1)?;
                let event = crate::events::trigger_event(trigger);
                if event.is_none() && trigger != 0 {
                    return Err(Error::ValueError); // level triggers aren't supported
                }
                let flex = self.input(pin)?;
                match event {
                    Some(event) => flex.listen(event),
                    None => flex.unlisten(),
                }
                crate::events::clear_pin(pin);
                Ok(None)
            }
            "timer_init" => {
                let slot = timer_slot(int(0)?)?;
                let period = u64::try_from(int(1)?).map_err(|_| Error::ValueError)?;
                if period == 0 {
                    return Err(Error::ValueError);
                }
                let now = since_boot().as_micros();
                self.events.start_timer(slot, period, int(2)? != 0, now);
                Ok(None)
            }
            "timer_deinit" => {
                self.events.stop_timer(timer_slot(int(0)?)?);
                Ok(None)
            }
            #[cfg(feature = "wifi")]
            "wifi_active" => {
                if let Some(on) = args.first() {
                    let on = on.int()? != 0;
                    if on || self.station.is_some() {
                        self.wifi_started()?.set_active(on)?;
                    }
                }
                Ok(Some(
                    self.station.as_ref().is_some_and(|s| s.is_active()) as i64
                ))
            }
            #[cfg(feature = "wifi")]
            "wifi_ap_active" => {
                if let Some(on) = args.first() {
                    let on = on.int()? != 0;
                    if on || self.station.is_some() {
                        self.wifi_started()?.set_ap_active(on)?;
                    }
                }
                Ok(Some(
                    self.station.as_ref().is_some_and(|s| s.is_ap_active()) as i64,
                ))
            }
            #[cfg(feature = "wifi")]
            "wifi_ap_config" => {
                let (ssid, key) = (args[0].str()?, args[1].str()?);
                let (auth, channel, hidden, max) = (int(2)?, int(3)?, int(4)? != 0, int(5)?);
                self.wifi_started()?
                    .configure_ap(ssid, key, auth, channel, hidden, max)
                    .map(|()| None)
            }
            #[cfg(feature = "wifi")]
            "wifi_connect" => {
                let key = match args.get(1) {
                    Some(k) => k.str()?,
                    None => "",
                };
                self.wifi()?.connect(args[0].str()?, key).map(|()| None)
            }
            #[cfg(feature = "wifi")]
            "wifi_disconnect" => self.wifi()?.disconnect().map(|()| None),
            #[cfg(feature = "wifi")]
            "wifi_status" => Ok(Some(match self.station.as_mut() {
                Some(station) => station.status(),
                None => crate::wifi::STAT_IDLE,
            })),
            #[cfg(feature = "wifi")]
            "wifi_rssi" => self.wifi()?.rssi().map(Some),
            #[cfg(feature = "wifi")]
            "wifi_ntp" => {
                let timeout = match args.get(1) {
                    Some(t) => non_negative(t.int()?)? as u64,
                    None => 1000,
                };
                self.wifi()?.ntp(args[0].str()?, timeout).map(Some)
            }
            "pwm_stop" => self.pwm_stop(pin_number(int(0)?)?).map(|()| None),
            "i2c_probe" => Ok(Some(self.i2c_probe(i2c_addr(int(0)?)?) as i64)),
            "i2c_write" => {
                let data = args[1].bytes()?;
                self.bus()?
                    .write(i2c_addr(int(0)?)?, data)
                    .map_err(|_| Error::IoError)?;
                Ok(Some(data.len() as i64))
            }
            "uart_init" => {
                let baud = match args.get(2) {
                    Some(b) => b.int()?,
                    None => 115_200,
                };
                self.uart_init(pin_number(int(0)?)?, pin_number(int(1)?)?, baud)
                    .map(|()| None)
            }
            "uart_write" => {
                let uart = self.uart.as_mut().ok_or(Error::IoError)?;
                let data = args[0].bytes()?;
                let mut sent = 0;
                while sent < data.len() {
                    sent += uart.write(&data[sent..]).map_err(|_| Error::IoError)?;
                }
                uart.flush().map_err(|_| Error::IoError)?;
                Ok(Some(data.len() as i64))
            }
            "uart_any" => {
                self.uart.as_ref().ok_or(Error::IoError)?;
                let waiting = esp_hal::peripherals::UART1::regs()
                    .status()
                    .read()
                    .rxfifo_cnt()
                    .bits();
                Ok(Some(waiting as i64))
            }
            "spi_init" => {
                let khz = match args.get(3) {
                    Some(k) => k.int()?,
                    None => 1000,
                };
                let mode = match args.get(4) {
                    Some(m) => m.int()?,
                    None => 0,
                };
                let pins = [
                    pin_number(int(0)?)?,
                    pin_number(int(1)?)?,
                    pin_number(int(2)?)?,
                ];
                self.spi_init(pins, khz, mode).map(|()| None)
            }
            "spi_write" => {
                let spi = self.spi.as_mut().ok_or(Error::IoError)?;
                spi.write(args[0].bytes()?).map_err(|_| Error::IoError)?;
                Ok(None)
            }
            "i2c_read_reg" => {
                let mut buf = [0u8; 1];
                self.i2c_read(int(0)?, int(1)?, &mut buf)?;
                Ok(Some(buf[0] as i64))
            }
            "i2c_read_reg16" => {
                let mut buf = [0u8; 2];
                self.i2c_read(int(0)?, int(1)?, &mut buf)?;
                Ok(Some(u16::from_be_bytes(buf) as i64))
            }
            "i2c_write_reg" => {
                let bytes = [byte(int(1)?)?, byte(int(2)?)?];
                self.bus()?
                    .write(i2c_addr(int(0)?)?, &bytes)
                    .map_err(|_| Error::IoError)?;
                Ok(None)
            }
            "wdt_enable" => {
                let timeout = non_negative(int(0)?)?;
                let rwdt = &mut self.rtc.rwdt;
                rwdt.set_stage_action(RwdtStage::Stage0, RwdtStageAction::ResetSystem);
                rwdt.set_timeout(RwdtStage::Stage0, Duration::from_millis(timeout as u64));
                rwdt.enable();
                rwdt.feed();
                self.wdt_enabled = true;
                Ok(None)
            }
            "wdt_feed" => {
                self.rtc.rwdt.feed();
                Ok(None)
            }
            "wdt_disable" => {
                self.rtc.rwdt.disable();
                self.wdt_enabled = false;
                Ok(None)
            }
            "reset" => esp_hal::system::software_reset(),
            "reset_cause" => Ok(Some(reset_cause())),
            "stack_peak" => Ok(Some(crate::stack::peak_and_repaint() as i64)),
            _ => Err(Error::UnknownIdentifier),
        }
    }

    /// Board functions that return bytes (`i2c_read`, `uart_read`, ...):
    /// they fill `out` and return how many bytes they have.
    pub fn call_bytes(
        &mut self,
        name: &str,
        args: &[Arg<'_>],
        out: &mut [u8],
    ) -> Result<usize, Error> {
        let int = |i: usize| args[i].int();
        let count = |n: i64| -> Result<usize, Error> {
            usize::try_from(n)
                .ok()
                .filter(|&n| n <= out.len())
                .ok_or(Error::ValueError)
        };
        match name {
            #[cfg(feature = "wifi")]
            "wifi_scan" => self.wifi()?.scan(out),
            #[cfg(feature = "wifi")]
            "wifi_ifconfig" => match self.station.as_mut() {
                Some(station) => Ok(station.ifconfig(out)),
                None => {
                    out[..16].fill(0);
                    Ok(16)
                }
            },
            #[cfg(feature = "wifi")]
            "wifi_ap_ifconfig" => Ok(self.wifi_started()?.ap_ifconfig(out)),
            #[cfg(feature = "wifi")]
            "wifi_ap_stations" => match self.station.as_mut() {
                Some(wifi) => Ok(wifi.ap_stations(out)),
                None => Ok(0),
            },
            #[cfg(feature = "wifi")]
            "wifi_ap_mac" => {
                out[..6].copy_from_slice(&crate::wifi::ap_mac());
                Ok(6)
            }
            #[cfg(feature = "wifi")]
            "wifi_mac" => {
                out[..6].copy_from_slice(&crate::wifi::sta_mac());
                Ok(6)
            }
            "unique_id" => {
                let mac = esp_hal::efuse::Efuse::mac_address();
                out[..mac.len()].copy_from_slice(&mac);
                Ok(mac.len())
            }
            "cpu_info" => Ok(cpu_info(out)),
            "i2c_scan" => {
                let mut n = 0;
                for addr in 0x08..0x78 {
                    if self.i2c_probe(addr) {
                        out[n] = addr;
                        n += 1;
                    }
                }
                Ok(n)
            }
            "i2c_read" => {
                let n = count(int(1)?)?;
                self.bus()?
                    .read(i2c_addr(int(0)?)?, &mut out[..n])
                    .map_err(|_| Error::IoError)?;
                Ok(n)
            }
            "i2c_write_read" => {
                let n = count(int(2)?)?;
                self.bus()?
                    .write_read(i2c_addr(int(0)?)?, args[1].bytes()?, &mut out[..n])
                    .map_err(|_| Error::IoError)?;
                Ok(n)
            }
            "uart_read" => {
                let n = count(args.first().map_or(Ok(64), |a| a.int())?)?;
                let timeout = args.get(1).map_or(Ok(0), |a| a.int())?;
                let deadline = now_ms() + non_negative(timeout)? as u64;
                let uart = self.uart.as_mut().ok_or(Error::IoError)?;
                let mut got = 0;
                loop {
                    got += uart
                        .read_buffered(&mut out[got..n])
                        .map_err(|_| Error::IoError)?;
                    if got == n || now_ms() >= deadline {
                        return Ok(got);
                    }
                    if crate::ABORT.load(core::sync::atomic::Ordering::Relaxed) {
                        return Err(Error::Interrupted);
                    }
                }
            }
            "spi_transfer" => {
                let data = args[0].bytes()?;
                let n = count(data.len() as i64)?;
                out[..n].copy_from_slice(data);
                let spi = self.spi.as_mut().ok_or(Error::IoError)?;
                spi.transfer(&mut out[..n]).map_err(|_| Error::IoError)?;
                Ok(n)
            }
            "spi_read" => {
                let n = count(int(0)?)?;
                let fill = match args.get(1) {
                    Some(f) => byte(f.int()?)?,
                    None => 0,
                };
                out[..n].fill(fill);
                let spi = self.spi.as_mut().ok_or(Error::IoError)?;
                spi.transfer(&mut out[..n]).map_err(|_| Error::IoError)?;
                Ok(n)
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }

    /// Takes a pin for a peripheral (UART, SPI): it must be in the
    /// allowlist, and it's no longer a plain GPIO or PWM output.
    fn take_pin(&mut self, pin: u8) -> Result<AnyPin<'static>, Error> {
        self.check_allowed(pin)?;
        self.pins[pin as usize] = None;
        if self.pwm_pins.contains(&Some(pin)) {
            self.pwm_stop(pin)?;
        }
        // SAFETY: the pin is in the allowlist (not used by the firmware) and
        // dropped from the GPIO table above, like the other pin functions do
        Ok(unsafe { AnyPin::steal(pin) })
    }

    /// `uart_init(tx, rx, baud)`: UART1 on any two allowed pins (the same
    /// pin for both is a loopback: the UART receives what it sends).
    fn uart_init(&mut self, tx: u8, rx: u8, baud: i64) -> Result<(), Error> {
        let baud = u32::try_from(baud)
            .ok()
            .filter(|b| (300..=5_000_000).contains(b));
        let baud = baud.ok_or(Error::ValueError)?;
        self.uart = None; // the old driver goes first: only one at a time
        let (tx, rx) = (self.take_pin(tx)?, self.take_pin(rx)?);
        // SAFETY: the previous UART1 driver was dropped just above, so this
        // is the only one
        let uart1 = unsafe { esp_hal::peripherals::UART1::steal() };
        let uart = Uart::new(uart1, UartConfig::default().with_baudrate(baud))
            .map_err(|_| Error::ValueError)?
            .with_tx(tx)
            .with_rx(rx);
        self.uart = Some(uart);
        Ok(())
    }

    /// `i2c_init(sda, scl, freq)`: the I2C bus on other pins or at another
    /// speed. Its boot pins (21, 22) are always allowed, others must be.
    fn i2c_init(&mut self, sda: u8, scl: u8, freq: i64) -> Result<(), Error> {
        let freq = u32::try_from(freq)
            .ok()
            .filter(|f| (1_000..=1_000_000).contains(f));
        let freq = freq.ok_or(Error::ValueError)?;
        if sda == scl {
            return Err(Error::ValueError);
        }
        let mut pin = |p: u8| {
            if p == I2C_SDA || p == I2C_SCL {
                // SAFETY: the boot I2C pins belong to this bus (never in the
                // allowlist, so no other driver has them)
                Ok(unsafe { AnyPin::steal(p) })
            } else {
                self.take_pin(p)
            }
        };
        let (sda, scl) = (pin(sda)?, pin(scl)?);
        // The old driver goes first: dropping it resets the peripheral, which
        // would stop a new driver made before
        self.i2c = None;
        // SAFETY: the previous I2C0 driver was dropped just above, so this
        // is the only one
        let i2c0 = unsafe { esp_hal::peripherals::I2C0::steal() };
        let i2c = I2c::new(
            i2c0,
            I2cConfig::default().with_frequency(Rate::from_hz(freq)),
        )
        .map_err(|_| Error::ValueError)?
        .with_sda(sda)
        .with_scl(scl);
        self.i2c = Some(i2c);
        Ok(())
    }

    /// `spi_init(sck, mosi, miso, khz, mode)`: SPI3 as master (SPI2 drives
    /// the TTGO's display); SPI2 on the ESP32-C3, its only general-purpose
    /// SPI. Chip select is up to the script (`pin_write`).
    fn spi_init(&mut self, [sck, mosi, miso]: [u8; 3], khz: i64, mode: i64) -> Result<(), Error> {
        let khz = u32::try_from(khz)
            .ok()
            .filter(|k| (10..=40_000).contains(k));
        let khz = khz.ok_or(Error::ValueError)?;
        let mode = match mode {
            0 => SpiMode::_0,
            1 => SpiMode::_1,
            2 => SpiMode::_2,
            3 => SpiMode::_3,
            _ => return Err(Error::ValueError),
        };
        self.spi = None;
        let (sck, mosi, miso) = (
            self.take_pin(sck)?,
            self.take_pin(mosi)?,
            self.take_pin(miso)?,
        );
        // SAFETY: the previous SPI driver was dropped just above
        #[cfg(not(feature = "esp32c3"))]
        let spi3 = unsafe { esp_hal::peripherals::SPI3::steal() };
        #[cfg(feature = "esp32c3")]
        let spi3 = unsafe { esp_hal::peripherals::SPI2::steal() };
        let config = SpiConfig::default()
            .with_frequency(Rate::from_khz(khz))
            .with_mode(mode);
        let spi = Spi::new(spi3, config)
            .map_err(|_| Error::ValueError)?
            .with_sck(sck)
            .with_mosi(mosi)
            .with_miso(miso);
        self.spi = Some(spi);
        Ok(())
    }

    /// Runs a function of a module from `board.rs` (`time`).
    pub fn call_module(
        &mut self,
        module: &str,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        match (module, name) {
            ("time" | "utime", "sleep") => {
                // Seconds, an int or a float (`time.sleep(0.5)`), to the microsecond
                let us = match args[0] {
                    Arg::Float(s) if s >= 0.0 && s < 1e12 => (s * 1e6) as i64,
                    Arg::Float(_) => return Err(Error::ValueError),
                    other => other
                        .int()?
                        .checked_mul(1_000_000)
                        .ok_or(Error::ValueError)?,
                };
                self.sleep_for_us(us.max(0) as u64).map(|()| None)
            }
            ("time" | "utime", "time") => Ok(Some(self.clock_us().div_euclid(1_000_000))),
            ("time" | "utime", "time_ns") => Ok(Some(self.clock_us().saturating_mul(1000))),
            ("time" | "utime", "ticks_add") => {
                Ok(Some(args[0].int()?.wrapping_add(args[1].int()?)))
            }
            // The other `time` functions are the flat ones with the same name
            ("time" | "utime", _) => self.call(name, args),
            _ => Err(Error::UnknownIdentifier),
        }
    }

    /// The clock: microseconds since 2000-01-01 (UTC).
    fn clock_us(&self) -> i64 {
        since_boot().as_micros() as i64 + self.clock_offset_us
    }

    pub fn sleep_ms(&mut self, ms: i64) -> Result<(), Error> {
        self.sleep_for_us(non_negative(ms)?.saturating_mul(1000) as u64)
    }

    /// Busy-waits until `us` have passed, in short steps: feeding the
    /// watchdog (so a long sleep doesn't trip it), polling Wi-Fi, and
    /// stopping with `Error::Pending` when an event is waiting for its
    /// Python callback (the interpreter runs it and resumes the sleep).
    pub fn sleep_for_us(&mut self, us: u64) -> Result<(), Error> {
        let deadline = since_boot().as_micros().saturating_add(us);
        let mut last_service = since_boot().as_micros();
        loop {
            // Ctrl-C on the serial port interrupts long sleeps too
            if crate::ABORT.load(core::sync::atomic::Ordering::Relaxed) {
                return Err(Error::Interrupted);
            }
            let now = since_boot().as_micros();
            if now >= deadline {
                return Ok(());
            }
            if self.events.on && self.events.waiting(now) {
                let left_ms = (deadline - now).div_ceil(1000);
                return Err(Error::Pending(left_ms.min(u32::MAX as u64) as u32));
            }
            // 1 ms steps while events may come (timers, pin interrupts)
            let step_us = if self.events.on {
                1000
            } else {
                SLEEP_CHUNK_MS as u64 * 1000
            };
            self.delay
                .delay_micros((deadline - now).min(step_us) as u32);
            if since_boot().as_micros() - last_service >= SLEEP_CHUNK_MS as u64 * 1000 {
                last_service = since_boot().as_micros();
                // A script waiting for Wi-Fi sleeps: DHCP and packets go on meanwhile
                #[cfg(feature = "wifi")]
                if let Some(station) = self.station.as_mut() {
                    station.poll();
                }
                if self.wdt_enabled {
                    self.rtc.rwdt.feed();
                }
            }
        }
    }

    /// The next event for the interpreter (`HardwareBridge::poll_event`).
    pub fn poll_event(&mut self) -> Option<i64> {
        self.events.poll(since_boot().as_micros())
    }

    /// Whether sleeps stop for events (`HardwareBridge::set_events`).
    pub fn set_events(&mut self, on: bool) {
        self.events.on = on;
    }

    /// `HardwareBridge::resume_sleep`.
    pub fn resume_sleep(&mut self, remaining_ms: u32) -> Result<(), Error> {
        self.sleep_for_us(remaining_ms as u64 * 1000)
    }

    /// A soft reset: no more pin interrupts and timers from the last session.
    pub fn soft_reset(&mut self) {
        for (pin, entry) in self.pins.iter_mut().enumerate() {
            if let Some((flex, _)) = entry.as_mut() {
                flex.unlisten();
                crate::events::clear_pin(pin as u8);
            }
        }
        self.events.reset();
        self.events.on = false;
    }

    /// Wi-Fi, once a script has activated it.
    #[cfg(feature = "wifi")]
    fn wifi(&mut self) -> Result<&mut crate::wifi::Wifi, Error> {
        self.station.as_mut().ok_or(Error::IoError) // "Wifi Not Started"
    }

    /// Wi-Fi, started now if it isn't yet (the scheduler and the driver).
    #[cfg(feature = "wifi")]
    fn wifi_started(&mut self) -> Result<&mut crate::wifi::Wifi, Error> {
        if self.station.is_none() {
            let parts = self.wifi_parts.take().ok_or(Error::IoError)?;
            self.station = Some(crate::wifi::Wifi::start(parts)?);
        }
        self.wifi()
    }

    /// The REPL is waiting for input: network work goes on (the access
    /// point's DHCP server, pings).
    pub fn idle(&mut self) {
        #[cfg(feature = "wifi")]
        if let Some(wifi) = self.station.as_mut() {
            wifi.poll();
        }
    }

    fn check_allowed(&self, pin: u8) -> Result<(), Error> {
        if self.allowed.contains(&pin) {
            Ok(())
        } else {
            Err(Error::ValueError)
        }
    }

    pub fn pin_mode(&mut self, pin: u8, mode: i64) -> Result<(), Error> {
        self.check_allowed(pin)?;
        self.release_pwm(pin);
        let flex = self.pins[pin as usize]
            .get_or_insert_with(|| (Flex::new(unsafe { AnyPin::steal(pin) }), PinMode::Input));

        let pull = match mode {
            OUT | OUT_OD | OUT_OD_PULLUP => {
                // GPIO34-39 are input-only on the ESP32
                if !chip::can_output(pin) {
                    return Err(Error::ValueError);
                }
                let config = match mode {
                    OUT => OutputConfig::default(),
                    OUT_OD => OutputConfig::default().with_drive_mode(DriveMode::OpenDrain),
                    _ => OutputConfig::default()
                        .with_drive_mode(DriveMode::OpenDrain)
                        .with_pull(Pull::Up),
                };
                flex.0.apply_output_config(&config);
                flex.0.set_output_enable(true);
                // Keep the input buffer on so `pin_read` returns the output level
                flex.0.set_input_enable(true);
                flex.1 = PinMode::Output;
                return Ok(());
            }
            IN => Pull::None,
            IN_PULLUP => Pull::Up,
            IN_PULLDOWN => Pull::Down,
            _ => return Err(Error::ValueError),
        };
        flex.0.set_output_enable(false);
        flex.0
            .apply_input_config(&InputConfig::default().with_pull(pull));
        flex.0.set_input_enable(true);
        flex.1 = PinMode::Input;
        Ok(())
    }

    /// The level of a pin (an input unless configured otherwise).
    pub fn read_pin(&mut self, pin: u8) -> Result<bool, Error> {
        self.check_allowed(pin)?;
        Ok(self.input(pin)?.is_high())
    }

    /// The pin as an output, switching it to `OUT` on first use.
    fn output(&mut self, pin: u8) -> Result<&mut Flex<'d>, Error> {
        if !matches!(self.pins[pin as usize], Some((_, PinMode::Output))) {
            self.pin_mode(pin, OUT)?;
        }
        Ok(&mut self.pins[pin as usize].as_mut().unwrap().0)
    }

    /// The pin for reading: outputs read back their level, unused pins
    /// become plain inputs.
    fn input(&mut self, pin: u8) -> Result<&mut Flex<'d>, Error> {
        if self.pins[pin as usize].is_none() {
            self.pin_mode(pin, IN)?;
        }
        Ok(&mut self.pins[pin as usize].as_mut().unwrap().0)
    }

    /// One reading as the ADC gives it: a raw 12-bit value on the ESP32,
    /// calibrated millivolts on the ESP32-S3 (`chip::ADC_READS_MV`).
    fn adc_sample(&mut self, pin: i64) -> Result<i64, Error> {
        let a = self.analog.as_mut().ok_or(Error::IoError)?;
        let value = match pin {
            p if p == ADC_PINS[0] as i64 => nb::block!(a.adc.read_oneshot(&mut a.pin_a)),
            p if p == ADC_PINS[1] as i64 => nb::block!(a.adc.read_oneshot(&mut a.pin_b)),
            p if p == ADC_PINS[2] as i64 => nb::block!(a.adc.read_oneshot(&mut a.pin_c)),
            _ => return Err(Error::ValueError),
        };
        value.map(i64::from).map_err(|_| Error::IoError)
    }

    /// With Wi-Fi running, sets ADC1 up again before a reading. The radio's
    /// calibration (when it starts, and while it runs) uses the same SAR ADC
    /// and changes its settings: the controller, attenuations and, on the
    /// ESP32-S3, the calibration code, which esp-hal writes only when it
    /// differs from the last one it wrote, so readings would stay off until
    /// a reset. ESP-IDF sets the ADC up before every one-shot reading too.
    /// Without Wi-Fi, nothing else touches the ADC.
    fn refresh_adc(&mut self) {
        #[cfg(feature = "wifi")]
        if self.station.is_some() {
            // The old driver goes first: the new one takes the same hardware
            self.analog = None;
            // SAFETY: the pins and ADC1 belonged to the driver just dropped,
            // which the board owned alone
            self.analog = Some(unsafe {
                Analog::new(
                    ADC1::steal(),
                    chip::AdcA::steal(),
                    chip::AdcB::steal(),
                    chip::AdcC::steal(),
                )
            });
        }
    }

    /// A raw 12-bit reading (0-4095). On the ESP32-S3 it comes from the
    /// calibrated millivolts, scaled to the 11 dB range.
    fn adc_read(&mut self, pin: i64) -> Result<i64, Error> {
        self.refresh_adc();
        let value = self.adc_sample(pin)?;
        if chip::ADC_READS_MV {
            return Ok((value * 4095 / chip::ADC_FULL_SCALE_MV).clamp(0, 4095));
        }
        Ok(value)
    }

    /// Average of `samples` readings in millivolts: converted with the
    /// chip's eFuse reference voltage on the ESP32 (see `raw_to_mv`), by
    /// esp-hal's calibration on the ESP32-S3.
    pub fn adc_mv(&mut self, pin: i64, samples: i64) -> Result<i64, Error> {
        if !(1..=MAX_ADC_SAMPLES).contains(&samples) {
            return Err(Error::ValueError);
        }
        self.refresh_adc();
        let mut total = 0;
        for _ in 0..samples {
            total += self.adc_sample(pin)?;
        }
        // Round to the nearest raw value before converting
        let value = (total + samples / 2) / samples;
        if chip::ADC_READS_MV {
            return Ok(value);
        }
        Ok(raw_to_mv(value as u32, adc_vref_mv()) as i64)
    }

    /// PWM on `pin` with `duty` as a 16-bit fraction (0 = off, 65535 = on).
    fn pwm(&mut self, pin: u8, freq: i64, duty: i64) -> Result<(), Error> {
        self.check_allowed(pin)?;
        if !chip::can_output(pin)
            || !(1..=40_000_000).contains(&freq)
            || !(0..=65535).contains(&duty)
        {
            return Err(Error::ValueError);
        }
        let slot = match self.pwm_pins.iter().position(|&p| p == Some(pin)) {
            Some(slot) => slot,
            None => self
                .pwm_pins
                .iter()
                .position(Option::is_none)
                .ok_or(Error::OutOfMemory)?,
        };
        // The LEDC peripheral now drives the pin instead of the GPIO driver
        self.pins[pin as usize] = None;

        let timer_number = [
            timer::Number::Timer0,
            timer::Number::Timer1,
            timer::Number::Timer2,
            timer::Number::Timer3,
        ][slot];
        let channel_number = [
            channel::Number::Channel0,
            channel::Number::Channel1,
            channel::Number::Channel2,
            channel::Number::Channel3,
        ][slot];

        // Use the finest duty resolution the frequency allows
        let mut timer = self.ledc.timer::<LowSpeed>(timer_number);
        let bits = DUTY_RESOLUTIONS.iter().find(|&&duty_bits| {
            timer
                .configure(timer::config::Config {
                    duty: duty_bits,
                    clock_source: timer::LSClockSource::APBClk,
                    frequency: Rate::from_hz(freq as u32),
                })
                .is_ok()
        });
        let bits = *bits.ok_or(Error::ValueError)? as u32;

        // The hardware keeps running after the driver objects are dropped
        let mut channel = self
            .ledc
            .channel(channel_number, unsafe { AnyPin::steal(pin) });
        channel
            .configure(channel::config::Config {
                timer: &timer,
                duty_pct: 0,
                drive_mode: DriveMode::PushPull,
            })
            .map_err(|_| Error::IoError)?;
        // The duty in the timer's resolution: 2^bits steps (2^bits = always on)
        let steps = 1u64 << bits;
        channel.set_duty_hw(((duty as u64 * steps + 32767) / 65535) as u32);
        self.pwm_pins[slot] = Some(pin);
        Ok(())
    }

    fn pwm_stop(&mut self, pin: u8) -> Result<(), Error> {
        self.check_allowed(pin)?;
        if !self.pwm_pins.contains(&Some(pin)) {
            return Err(Error::ValueError);
        }
        self.release_pwm(pin);
        Ok(())
    }

    /// Hands a PWM pin back to the GPIO driver as a low output.
    fn release_pwm(&mut self, pin: u8) {
        let Some(slot) = self.pwm_pins.iter().position(|&p| p == Some(pin)) else {
            return;
        };
        self.pwm_pins[slot] = None;
        let mut flex = Flex::new(unsafe { AnyPin::steal(pin) });
        flex.apply_output_config(&OutputConfig::default());
        flex.set_level(Level::Low);
        flex.set_output_enable(true);
        flex.set_input_enable(true);
        self.pins[pin as usize] = Some((flex, PinMode::Output));
    }

    fn bus(&mut self) -> Result<&mut I2c<'d, Blocking>, Error> {
        self.i2c.as_mut().ok_or(Error::IoError)
    }

    fn i2c_probe(&mut self, addr: u8) -> bool {
        self.bus().is_ok_and(|bus| bus.write(addr, &[]).is_ok())
    }

    fn i2c_read(&mut self, addr: i64, reg: i64, buf: &mut [u8]) -> Result<(), Error> {
        self.bus()?
            .write_read(i2c_addr(addr)?, &[byte(reg)?], buf)
            .map_err(|_| Error::IoError)
    }
}

/// Duty resolutions to try for PWM, finest first. Low frequencies (servos at
/// 50 Hz) need many bits, high ones (100 kHz+) only few.
const DUTY_RESOLUTIONS: [timer::config::Duty; 10] = {
    use timer::config::Duty::*;
    [
        Duty14Bit, Duty13Bit, Duty12Bit, Duty11Bit, Duty10Bit, Duty9Bit, Duty8Bit, Duty6Bit,
        Duty4Bit, Duty2Bit,
    ]
};

const DEFAULT_ADC_SAMPLES: i64 = 16;
const MAX_ADC_SAMPLES: i64 = 1024;

/// The ADC reference voltage of this chip in mV, from eFuse. The factory
/// stores the deviation from 1100 mV in 7 mV steps (sign-magnitude, 5 bits);
/// chips without it read as 0 deviation, i.e. 1100 mV.
#[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
pub fn adc_vref_mv() -> u32 {
    1100 // the nominal value: the S3's calibration is per attenuation (esp-hal applies it)
}

#[cfg(feature = "esp32")]
pub fn adc_vref_mv() -> u32 {
    let bits: u8 = esp_hal::efuse::Efuse::read_field_le(esp_hal::efuse::ADC_VREF);
    let magnitude = (bits & 0x0F) as i32;
    let deviation = if bits & 0x10 != 0 {
        -magnitude
    } else {
        magnitude
    };
    (1100 + deviation * 7) as u32
}

/// Raw 12-bit reading at 11 dB attenuation to millivolts, with ESP-IDF's
/// linear characterization for ADC1 (`esp_adc_cal`, "Vref" mode):
/// `mv = (vref * 196602 / 4096 * raw + 32768) / 65536 + 142`.
/// ESP-IDF adds a lookup table for the top of the 11 dB range, which this
/// leaves out, so readings above ~2.6 V are a few percent less accurate.
pub fn raw_to_mv(raw: u32, vref_mv: u32) -> u32 {
    const SCALE_11DB: u64 = 196_602;
    const OFFSET_11DB: u64 = 142;
    let coeff_a = vref_mv as u64 * SCALE_11DB / 4096;
    ((coeff_a * raw as u64 + 32_768) / 65_536 + OFFSET_11DB) as u32
}

/// `cpu_info()`: what `machine.lscpu()` shows, as `key=value` lines (the
/// bridge passes bytes, not dicts; machine.py turns them into a dict).
fn cpu_info(out: &mut [u8]) -> usize {
    use chip::about;
    use core::fmt::Write;

    struct Cursor<'b>(&'b mut [u8], usize);
    impl Write for Cursor<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let end = (self.1 + s.len()).min(self.0.len());
            self.0[self.1..end].copy_from_slice(&s.as_bytes()[..end - self.1]);
            self.1 = end;
            Ok(())
        }
    }
    let mut c = Cursor(out, 0);
    let rev = esp_hal::efuse::Efuse::chip_revision();
    let mac = esp_hal::efuse::Efuse::mac_address();
    let level = if cfg!(feature = "minimal") {
        "minimal"
    } else if cfg!(feature = "medium") {
        "medium"
    } else {
        "full"
    };
    let _ = write!(
        c,
        "arch={}\nmodel={}\ncore={}\nrev=v{}.{}\ncores={}\nmhz={}\nmax_mhz={}\nflags={}\n\
         flash={}\nsram={}\nstack={}\n",
        about::ARCH,
        chip::NAME,
        about::CORE,
        rev / 100,
        rev % 100,
        about::CORES,
        CPU_CLOCK as u32,
        about::MAX_MHZ,
        about::FLAGS,
        crate::storage::flash_size(),
        about::SRAM,
        crate::stack::size(),
    );
    let _ =
        write!(
        c,
        "mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\nboard={}\nfirmware={}\nbuild={}{}{}{}\n\
         uptime_ms={}\nreset={}\n",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        chip::BOARD,
        env!("CARGO_PKG_VERSION"),
        level,
        if cfg!(feature = "wifi") { " wifi" } else { "" },
        if cfg!(feature = "tft") { " tft" } else { "" },
        if cfg!(feature = "float32") { " float32" } else { "" },
        now_ms(),
        reset_cause(),
    );
    #[cfg(feature = "wifi")]
    let _ = write!(
        c,
        "wifi_heap={}\nwifi_heap_free={}\n",
        esp_alloc::HEAP.used() + esp_alloc::HEAP.free(),
        esp_alloc::HEAP.free()
    );
    c.1
}

/// Why the chip started, as one of the `*_RESET` constants.
pub fn reset_cause() -> i64 {
    use SocResetReason::*;
    match esp_hal::system::reset_reason() {
        Some(ChipPowerOn | SysBrownOut) => PWRON_RESET,
        #[cfg(feature = "esp32")]
        Some(CoreMwdt0 | CoreMwdt1 | CoreRtcWdt | CpuMwdt0 | Cpu0RtcWdt | SysRtcWdt) => WDT_RESET,
        #[cfg(feature = "esp32")]
        Some(CoreSw | Cpu0Sw) => SOFT_RESET,
        #[cfg(feature = "esp32s3")]
        Some(
            CoreMwdt0 | CoreMwdt1 | CoreRtcWdt | CpuMwdt0 | CpuMwdt1 | CpuRtcWdt | SysRtcWdt
            | SysSuperWdt,
        ) => WDT_RESET,
        // The chip's USB port resets it too (espflash, the USB-Serial/JTAG)
        #[cfg(feature = "esp32s3")]
        Some(CoreSw | CpuSw | CoreUsbUart | CoreUsbJtag) => SOFT_RESET,
        #[cfg(feature = "esp32c3")]
        Some(
            CoreMwdt0 | CoreMwdt1 | CoreRtcWdt | Cpu0Mwdt0 | Cpu0Mwdt1 | Cpu0RtcWdt | SysRtcWdt
            | SysSuperWdt,
        ) => WDT_RESET,
        #[cfg(feature = "esp32c3")]
        Some(CoreSw | Cpu0Sw | CoreUsbUart | CoreUsbJtag) => SOFT_RESET,
        Some(CoreDeepSleep) => DEEPSLEEP_RESET,
        _ => HARD_RESET,
    }
}

/// Milliseconds since boot.
pub fn now_ms() -> u64 {
    since_boot().as_millis()
}

/// MicroPython's timer ids: 0-3 (the ESP32's hardware timers), and -1
/// (a virtual timer) as 4.
fn timer_slot(id: i64) -> Result<usize, Error> {
    match id {
        0..=3 => Ok(id as usize),
        -1 => Ok(4),
        _ => Err(Error::ValueError),
    }
}

fn since_boot() -> Duration {
    Instant::now().duration_since_epoch()
}

fn non_negative(n: i64) -> Result<i64, Error> {
    if n < 0 || n > u32::MAX as i64 {
        Err(Error::ValueError)
    } else {
        Ok(n)
    }
}

fn pin_number(n: i64) -> Result<u8, Error> {
    u8::try_from(n)
        .ok()
        .filter(|&p| (p as usize) < chip::GPIO_COUNT && chip::gpio_exists(p))
        .ok_or(Error::ValueError)
}

fn i2c_addr(n: i64) -> Result<u8, Error> {
    u8::try_from(n)
        .ok()
        .filter(|&a| a < 0x80)
        .ok_or(Error::ValueError)
}

fn byte(n: i64) -> Result<u8, Error> {
    u8::try_from(n).map_err(|_| Error::ValueError)
}
