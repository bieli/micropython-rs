//! Hardware functions shared by every ESP32 board build:
//! time, GPIO, ADC, PWM, I2C and the watchdog.
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
        channel::{self, ChannelIFace},
        timer::{self, TimerIFace},
        LSGlobalClkSource, Ledc, LowSpeed,
    },
    peripherals::{ADC1, GPIO21, GPIO22, GPIO34, GPIO36, GPIO39, I2C0, LEDC, LPWR},
    rtc_cntl::{Rtc, RwdtStage, RwdtStageAction, SocResetReason},
    time::{Duration, Instant, Rate},
    Blocking,
};
use micropython_rs::{Arg, Error, HwFn, Module};

/// Pin modes for `pin_mode(pin, mode)`; also exported as script constants.
pub const OUT: i64 = 1;
pub const IN: i64 = 2;
pub const IN_PULLUP: i64 = 3;
pub const IN_PULLDOWN: i64 = 4;

/// `reset_cause()` results, with MicroPython's `machine` module values.
pub const PWRON_RESET: i64 = 1;
pub const HARD_RESET: i64 = 2;
pub const WDT_RESET: i64 = 3;
pub const DEEPSLEEP_RESET: i64 = 4;
pub const SOFT_RESET: i64 = 5;

/// `import time`: MicroPython's `time` module names for the time functions.
pub const TIME_MODULE: Module = Module {
    name: "time",
    doc: "Delays and timestamps (MicroPython names)",
    functions: &[
        HwFn::new("sleep", "s", "Wait s seconds"),
        HwFn::new("sleep_ms", "ms", "Wait ms milliseconds"),
        HwFn::new("sleep_us", "us", "Wait us microseconds"),
        HwFn::new("ticks_ms", "", "Milliseconds since boot"),
        HwFn::new("ticks_us", "", "Microseconds since boot"),
        HwFn::new("ticks_diff", "new, old", "new - old"),
    ],
    constants: &[],
};

/// Pins wired to the ADC: the input-only GPIO34/36/39, so analog inputs
/// never collide with digital outputs.
pub const ADC_PINS: [u8; 3] = [34, 36, 39];
/// I2C bus pins (the ESP32 defaults).
pub const I2C_SDA: u8 = 21;
pub const I2C_SCL: u8 = 22;

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
            micropython_rs::HwFn::new("pin_mode", "pin, mode", "mode: OUT, IN, IN_PULLUP, IN_PULLDOWN"),
            micropython_rs::HwFn::new("pin_write", "pin, value", "Set an output pin to 0 or 1"),
            micropython_rs::HwFn::new("pin_read", "pin", "Read a pin: 0 or 1"),
            micropython_rs::HwFn::new("pin_toggle", "pin", "Invert an output pin"),
            micropython_rs::HwFn::new("adc_read", "pin", "Raw 12-bit ADC value (pins 34, 36, 39)"),
            micropython_rs::HwFn::with_args("adc_mv", 1, 2, "pin, [samples]", "Calibrated millivolts, averaged (default 16 samples)"),
            micropython_rs::HwFn::new("adc_vref", "", "ADC reference voltage from eFuse, mV"),
            micropython_rs::HwFn::new("pwm", "pin, freq, duty", "PWM output, duty in percent (0-100)"),
            micropython_rs::HwFn::new("pwm_stop", "pin", "Stop PWM on a pin"),
            micropython_rs::HwFn::new("i2c_probe", "addr", "True if a device answers at addr"),
            micropython_rs::HwFn::new("i2c_scan", "", "Number of devices on the bus"),
            micropython_rs::HwFn::new("i2c_read_reg", "addr, reg", "Read one register byte"),
            micropython_rs::HwFn::new("i2c_read_reg16", "addr, reg", "Read a big-endian 16-bit register"),
            micropython_rs::HwFn::new("i2c_write_reg", "addr, reg, value", "Write one register byte"),
            micropython_rs::HwFn::new("wdt_enable", "timeout_ms", "Reset the board unless fed in time"),
            micropython_rs::HwFn::new("wdt_feed", "", "Feed the watchdog"),
            micropython_rs::HwFn::new("wdt_disable", "", "Turn the watchdog off"),
            micropython_rs::HwFn::new("reset", "", "Restart the board"),
            micropython_rs::HwFn::new("reset_cause", "", "PWRON_RESET, WDT_RESET, SOFT_RESET, ..."),
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
            ("HIGH", 1),
            ("LOW", 0),
            ("PWRON_RESET", $crate::board::PWRON_RESET),
            ("HARD_RESET", $crate::board::HARD_RESET),
            ("WDT_RESET", $crate::board::WDT_RESET),
            ("DEEPSLEEP_RESET", $crate::board::DEEPSLEEP_RESET),
            ("SOFT_RESET", $crate::board::SOFT_RESET),
            $($extra),*
        ]
    };
}

/// The peripherals the board module needs.
pub struct BoardPeripherals<'d> {
    pub adc1: ADC1<'d>,
    pub gpio34: GPIO34<'d>,
    pub gpio36: GPIO36<'d>,
    pub gpio39: GPIO39<'d>,
    pub ledc: LEDC<'d>,
    pub i2c0: I2C0<'d>,
    pub sda: GPIO21<'d>,
    pub scl: GPIO22<'d>,
    pub lpwr: LPWR<'d>,
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
    pins: [Option<(Flex<'d>, PinMode)>; 40],
    adc: Adc<'d, ADC1<'d>, Blocking>,
    adc34: AdcPin<GPIO34<'d>, ADC1<'d>>,
    adc36: AdcPin<GPIO36<'d>, ADC1<'d>>,
    adc39: AdcPin<GPIO39<'d>, ADC1<'d>>,
    ledc: Ledc<'d>,
    pwm_pins: [Option<u8>; PWM_SLOTS],
    i2c: I2c<'d, Blocking>,
    rtc: Rtc<'d>,
    wdt_enabled: bool,
}

impl<'d> Board<'d> {
    /// `allowed` lists the GPIO numbers scripts may use with `pin_*` and `pwm`.
    pub fn new(p: BoardPeripherals<'d>, allowed: &'static [u8]) -> Self {
        let mut adc_config = AdcConfig::new();
        let adc34 = adc_config.enable_pin(p.gpio34, Attenuation::_11dB);
        let adc36 = adc_config.enable_pin(p.gpio36, Attenuation::_11dB);
        let adc39 = adc_config.enable_pin(p.gpio39, Attenuation::_11dB);
        let adc = Adc::new(p.adc1, adc_config);

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
            pins: [const { None }; 40],
            adc,
            adc34,
            adc36,
            adc39,
            ledc,
            pwm_pins: [None; PWM_SLOTS],
            i2c,
            rtc: Rtc::new(p.lpwr),
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
            "pwm" => self
                .pwm(pin_number(int(0)?)?, int(1)?, int(2)?)
                .map(|()| None),
            "pwm_stop" => self.pwm_stop(pin_number(int(0)?)?).map(|()| None),
            "i2c_probe" => Ok(Some(self.i2c_probe(i2c_addr(int(0)?)?) as i64)),
            "i2c_scan" => Ok(Some(
                (0x08..0x78).filter(|&addr| self.i2c_probe(addr)).count() as i64,
            )),
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
                self.i2c
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
            _ => Err(Error::UnknownIdentifier),
        }
    }

    /// Runs a function of a module from `board.rs` (`time`).
    pub fn call_module(
        &mut self,
        module: &str,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        match (module, name) {
            ("time", "sleep") => {
                let ms = args[0].int()?.checked_mul(1000).ok_or(Error::ValueError)?;
                self.sleep_ms(ms).map(|()| None)
            }
            // The other `time` functions are the flat ones with the same name
            ("time", _) => self.call(name, args),
            _ => Err(Error::UnknownIdentifier),
        }
    }

    /// Busy-waits in short chunks, feeding the watchdog in between, so a
    /// long `sleep_ms` doesn't trip it.
    pub fn sleep_ms(&mut self, ms: i64) -> Result<(), Error> {
        let mut remaining = non_negative(ms)?;
        while remaining > 0 {
            // Ctrl-C on the serial port interrupts long sleeps too
            if crate::ABORT.load(core::sync::atomic::Ordering::Relaxed) {
                return Err(Error::Interrupted);
            }
            let chunk = remaining.min(SLEEP_CHUNK_MS);
            self.delay.delay_millis(chunk as u32);
            remaining -= chunk;
            if self.wdt_enabled {
                self.rtc.rwdt.feed();
            }
        }
        Ok(())
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
            OUT => {
                // GPIO34-39 are input-only on the ESP32
                if pin >= 34 {
                    return Err(Error::ValueError);
                }
                flex.0.apply_output_config(&OutputConfig::default());
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

    fn adc_read(&mut self, pin: i64) -> Result<i64, Error> {
        let value = match pin {
            34 => nb::block!(self.adc.read_oneshot(&mut self.adc34)),
            36 => nb::block!(self.adc.read_oneshot(&mut self.adc36)),
            39 => nb::block!(self.adc.read_oneshot(&mut self.adc39)),
            _ => return Err(Error::ValueError),
        };
        value.map(i64::from).map_err(|_| Error::IoError)
    }

    /// Average of `samples` readings, converted to millivolts with the chip's
    /// eFuse reference voltage (see `raw_to_mv`).
    pub fn adc_mv(&mut self, pin: i64, samples: i64) -> Result<i64, Error> {
        if !(1..=MAX_ADC_SAMPLES).contains(&samples) {
            return Err(Error::ValueError);
        }
        let mut total = 0;
        for _ in 0..samples {
            total += self.adc_read(pin)?;
        }
        // Round to the nearest raw value before converting
        let raw = (total + samples / 2) / samples;
        Ok(raw_to_mv(raw as u32, adc_vref_mv()) as i64)
    }

    fn pwm(&mut self, pin: u8, freq: i64, duty: i64) -> Result<(), Error> {
        self.check_allowed(pin)?;
        if pin >= 34 || !(1..=40_000_000).contains(&freq) || !(0..=100).contains(&duty) {
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
        let configured = DUTY_RESOLUTIONS.iter().any(|&duty_bits| {
            timer
                .configure(timer::config::Config {
                    duty: duty_bits,
                    clock_source: timer::LSClockSource::APBClk,
                    frequency: Rate::from_hz(freq as u32),
                })
                .is_ok()
        });
        if !configured {
            return Err(Error::ValueError);
        }

        // The hardware keeps running after the driver objects are dropped
        let mut channel = self
            .ledc
            .channel(channel_number, unsafe { AnyPin::steal(pin) });
        channel
            .configure(channel::config::Config {
                timer: &timer,
                duty_pct: duty as u8,
                drive_mode: DriveMode::PushPull,
            })
            .map_err(|_| Error::IoError)?;
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

    fn i2c_probe(&mut self, addr: u8) -> bool {
        self.i2c.write(addr, &[]).is_ok()
    }

    fn i2c_read(&mut self, addr: i64, reg: i64, buf: &mut [u8]) -> Result<(), Error> {
        self.i2c
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

/// Why the chip started, as one of the `*_RESET` constants.
pub fn reset_cause() -> i64 {
    use SocResetReason::*;
    match esp_hal::system::reset_reason() {
        Some(ChipPowerOn | SysBrownOut) => PWRON_RESET,
        Some(CoreMwdt0 | CoreMwdt1 | CoreRtcWdt | CpuMwdt0 | Cpu0RtcWdt | SysRtcWdt) => WDT_RESET,
        Some(CoreSw | Cpu0Sw) => SOFT_RESET,
        Some(CoreDeepSleep) => DEEPSLEEP_RESET,
        _ => HARD_RESET,
    }
}

/// Milliseconds since boot.
pub fn now_ms() -> u64 {
    since_boot().as_millis()
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
        .filter(|&p| p < 40)
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
