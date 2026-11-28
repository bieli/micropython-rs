//! An analog microphone as the native module `mic`: a MEMS (or electret)
//! microphone with an amplifier whose output swings around a DC bias, read
//! by the chip's ADC at a fixed sample rate. That's most breakout boards
//! (MAX4466, MAX9814, SPW2430, an MSM381 like the XIAO MG24 Sense's) and any
//! other analog signal source:
//!
//! ```text
//! import mic
//! mic.init()                     # the board's microphone, 16 kHz
//! mic.init(pin, power, rate)     # another one (power -1: no power pin)
//! pcm = mic.record(8000)         # 8000 samples: signed 16-bit LE, DC removed
//! open('a.wav', 'wb').write(mic.wav(8000))   # the same as a WAV file
//! rms, peak, dc = mic.level(100) # 100 ms: the signal in mV (rms, peak), its bias
//! mic.raw(64)                    # the ADC values themselves (a list)
//! mic.rate()                     # the real sample rate, Hz
//! mic.off()                      # its power off
//! ```
//!
//! The samples: the ADC's values with their mean (the bias) taken off, scaled
//! so the ADC's full range is the 16-bit range (a 12-bit ADC: times 16), then
//! multiplied by `gain` (default 1; clipped to +-32767). The first call (or
//! the first after `off`) starts the microphone with the last pins and rate.
//!
//! This part is the same on every chip; a board provides the ADC sampling
//! (`MicHw`), its default pins (`Mic::new`) and the pin bookkeeping (it
//! asks `Mic::pins_for` which pins a call will take).

use micropython_rs::{Arg, Error, HwFn, Returns};

/// The module's functions (the board adds its constants, e.g. `PIN`).
pub const FUNCTIONS: &[HwFn] = &[
    HwFn::with_args(
        "init",
        0,
        3,
        "[pin], [power], [rate]",
        "Starts a microphone (power pin -1: none; default rate 16000 Hz); the real rate",
    )
    .returning_values(),
    HwFn::with_args(
        "record",
        1,
        2,
        "n, [gain]",
        "n samples: bytes, signed 16-bit little-endian, DC removed",
    )
    .returning_values(),
    HwFn::with_args(
        "wav",
        1,
        2,
        "n, [gain]",
        "n samples as a WAV file (16-bit mono): bytes",
    )
    .returning_values(),
    HwFn::with_args(
        "level",
        0,
        1,
        "[ms]",
        "(rms, peak, dc) in mV over ms (default 100)",
    )
    .returning_values(),
    HwFn::new("raw", "n", "n ADC values (a list; n up to 1024)").returning_values(),
    HwFn::new("rate", "", "The real sample rate, Hz").returning_values(),
    HwFn::new("off", "", "Turns the microphone's power off").returning_values(),
];

/// What a board provides: power, an analog input and timed ADC conversions.
pub trait MicHw {
    /// Drives the microphone's power pin (on: powered).
    fn power(&mut self, pin: i64, on: bool) -> Result<(), Error>;
    /// Makes `pin` an analog input.
    fn analog_input(&mut self, pin: i64) -> Result<(), Error>;
    /// The sample rate the hardware uses when asked for `rate_hz` (its
    /// timer's period is a whole number of clock cycles); `ValueError` if
    /// it can't sample at about that rate.
    fn actual_rate(&mut self, rate_hz: u32) -> Result<f64, Error>;
    /// `n` conversions of `pin`, evenly spaced at `rate_hz`, each given to
    /// `each` as it arrives (0 to `full_scale().0`).
    fn sample(
        &mut self,
        pin: i64,
        rate_hz: u32,
        n: usize,
        each: &mut dyn FnMut(u16),
    ) -> Result<(), Error>;
    /// The ADC's largest value (4095 for 12 bits) and its voltage, mV.
    fn full_scale(&mut self) -> (u16, u32);
    fn delay_ms(&mut self, ms: u32);
}

/// The longest `raw` list.
pub const MAX_RAW: usize = 1024;
const RATES: core::ops::RangeInclusive<i64> = 1000..=48_000;
const MAX_GAIN: i64 = 256;

/// The microphone's settings and whether it's on.
pub struct Mic {
    pin: i64,
    power: Option<i64>,
    rate: u32,
    /// Time from power-on to a settled output (its bias), ms.
    settle_ms: u32,
    on: bool,
}

impl Mic {
    /// The board's microphone: its pins, rate and settling time.
    pub const fn new(pin: i64, power: Option<i64>, rate: u32, settle_ms: u32) -> Self {
        Self {
            pin,
            power,
            rate,
            settle_ms,
            on: false,
        }
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// The signal pin and the power pin (if any) of the current settings.
    pub fn pins(&self) -> (i64, Option<i64>) {
        (self.pin, self.power)
    }

    /// The pins call `name` will take, if it starts the microphone (`init`,
    /// or any call while it's off): the board frees them first.
    pub fn pins_for(
        &self,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<(i64, Option<i64>)>, Error> {
        if name == "init" {
            let (pin, power, _) = self.init_args(args)?;
            Ok(Some((pin, power)))
        } else if !self.on && name != "off" {
            Ok(Some((self.pin, self.power)))
        } else {
            Ok(None)
        }
    }

    /// Marks it off without touching the hardware (the board took its pins).
    pub fn forget(&mut self) {
        self.on = false;
    }

    fn init_args(&self, args: &[Arg<'_>]) -> Result<(i64, Option<i64>, u32), Error> {
        let pin = match args.first() {
            Some(a) => a.int()?,
            None => self.pin,
        };
        let power = match args.get(1) {
            Some(a) => match a.int()? {
                -1 => None,
                p => Some(p),
            },
            None => self.power,
        };
        let rate = match args.get(2) {
            Some(a) => {
                let r = a.int()?;
                if !RATES.contains(&r) {
                    return Err(Error::ValueError);
                }
                r as u32
            }
            None => self.rate,
        };
        Ok((pin, power, rate))
    }

    fn start(&mut self, hw: &mut dyn MicHw, args: &[Arg<'_>]) -> Result<f64, Error> {
        let (pin, power, rate) = self.init_args(args)?;
        let actual = hw.actual_rate(rate)?;
        hw.analog_input(pin)?;
        if let Some(p) = power {
            hw.power(p, true)?;
            hw.delay_ms(self.settle_ms);
        }
        *self = Self {
            pin,
            power,
            rate,
            settle_ms: self.settle_ms,
            on: true,
        };
        Ok(actual)
    }

    /// Runs `mic.<name>(*args)`.
    pub fn call(
        &mut self,
        hw: &mut dyn MicHw,
        name: &str,
        args: &[Arg<'_>],
        out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        match name {
            "init" => {
                let actual = self.start(hw, args)?;
                return out.float(actual);
            }
            "off" => {
                if let (true, Some(p)) = (self.on, self.power) {
                    hw.power(p, false)?;
                }
                self.on = false;
                return Ok(());
            }
            _ => {}
        }
        if !self.on {
            self.start(hw, &[])?;
        }
        let (pin, rate) = (self.pin, self.rate);
        match name {
            "rate" => out.float(hw.actual_rate(rate)?),
            "record" | "wav" => {
                let n = count(args[0].int()?, usize::MAX / 4)?;
                let gain = match args.get(1) {
                    Some(g) => g.int()?,
                    None => 1,
                };
                if !(1..=MAX_GAIN).contains(&gain) {
                    return Err(Error::ValueError);
                }
                let header = if name == "wav" { WAV_HEADER } else { 0 };
                let actual = hw.actual_rate(rate)?;
                let (full, _) = hw.full_scale();
                out.bytes_with(header + 2 * n, |buf| {
                    let (head, pcm) = buf.split_at_mut(header);
                    if header > 0 {
                        wav_header(head, (actual + 0.5) as u32, pcm.len() as u32);
                    }
                    record(hw, pin, rate, pcm, full, gain as i32)
                })
            }
            "level" => {
                let ms = match args.first() {
                    Some(m) => m.int()?,
                    None => 100,
                };
                if !(1..=10_000).contains(&ms) {
                    return Err(Error::ValueError);
                }
                let n = ((rate as u64 * ms as u64 / 1000) as usize).max(2);
                let mut stats = Stats::default();
                hw.sample(pin, rate, n, &mut |v| stats.add(v))?;
                let (full, full_mv) = hw.full_scale();
                let mv = |counts: f64| counts * full_mv as f64 / full as f64;
                out.begin_tuple()?;
                out.float(mv(stats.rms()))?;
                out.float(mv(stats.peak()))?;
                out.float(mv(stats.mean()))?;
                out.end()
            }
            "raw" => {
                let n = count(args[0].int()?, MAX_RAW)?;
                let mut values = [0u16; MAX_RAW];
                let mut i = 0;
                hw.sample(pin, rate, n, &mut |v| {
                    values[i] = v;
                    i += 1;
                })?;
                out.begin_list()?;
                for &v in &values[..n] {
                    out.int(v as i64)?;
                }
                out.end()
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }
}

/// `n` as a sample count, 0 to `max`.
fn count(n: i64, max: usize) -> Result<usize, Error> {
    usize::try_from(n)
        .ok()
        .filter(|&n| n <= max)
        .ok_or(Error::ValueError)
}

/// `pcm.len() / 2` samples into `pcm`: first the raw values, then, with
/// their mean known, each becomes (value - mean) * (65536 / ADC range) *
/// gain, clipped, as signed 16-bit little-endian.
fn record(
    hw: &mut dyn MicHw,
    pin: i64,
    rate: u32,
    pcm: &mut [u8],
    full: u16,
    gain: i32,
) -> Result<(), Error> {
    let n = pcm.len() / 2;
    if n == 0 {
        return Ok(());
    }
    let mut i = 0;
    let mut sum = 0u64;
    hw.sample(pin, rate, n, &mut |v| {
        if i < n {
            pcm[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
            sum += v as u64;
            i += 1;
        }
    })?;
    // Fixed point with 8 fractional bits: the mean is rarely a whole count
    let scale = 65536 / (full as i64 + 1); // 16 for a 12-bit ADC
    let mean = ((sum << 8) / n as u64) as i64;
    for s in pcm.as_chunks_mut::<2>().0 {
        let v = u16::from_le_bytes([s[0], s[1]]) as i64;
        let x = (((v << 8) - mean) * scale * gain as i64) >> 8;
        let x = x.clamp(-32767, 32767) as i16;
        s.copy_from_slice(&x.to_le_bytes());
    }
    Ok(())
}

/// A WAV file's header: 44 bytes.
pub const WAV_HEADER: usize = 44;

/// The 44-byte header of a 16-bit mono PCM WAV file of `data_len` bytes.
pub fn wav_header(h: &mut [u8], rate: u32, data_len: u32) {
    let mut at = 0;
    let mut put = |b: &[u8]| {
        h[at..at + b.len()].copy_from_slice(b);
        at += b.len();
    };
    put(b"RIFF");
    put(&(36 + data_len).to_le_bytes());
    put(b"WAVEfmt ");
    put(&16u32.to_le_bytes()); // the fmt chunk's size
    put(&1u16.to_le_bytes()); // PCM
    put(&1u16.to_le_bytes()); // mono
    put(&rate.to_le_bytes());
    put(&(rate * 2).to_le_bytes()); // bytes per second
    put(&2u16.to_le_bytes()); // bytes per frame
    put(&16u16.to_le_bytes()); // bits per sample
    put(b"data");
    put(&data_len.to_le_bytes());
}

/// Running sums for `level`, without a buffer.
#[derive(Default)]
struct Stats {
    n: u64,
    sum: u64,
    squares: u64,
    min: u16,
    max: u16,
}

impl Stats {
    fn add(&mut self, v: u16) {
        if self.n == 0 || v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
        self.n += 1;
        self.sum += v as u64;
        self.squares += v as u64 * v as u64;
    }

    fn mean(&self) -> f64 {
        self.sum as f64 / self.n.max(1) as f64
    }

    /// The standard deviation (the rms of the signal without its bias):
    /// sqrt(n * sum(v^2) - sum(v)^2) / n, the root in integers (no_std
    /// has no floating-point sqrt), 4 fractional bits.
    fn rms(&self) -> f64 {
        let n = self.n.max(1) as u128;
        let var_n2 = (n * self.squares as u128).saturating_sub(self.sum as u128 * self.sum as u128);
        (var_n2 << 8).isqrt() as f64 / 16.0 / n as f64
    }

    /// The largest distance from the mean.
    fn peak(&self) -> f64 {
        let m = self.mean();
        (self.max as f64 - m).max(m - self.min as f64)
    }
}
