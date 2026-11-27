//! A native sensor module in Rust: a Bosch BME280 (temperature, pressure,
//! humidity on I2C) as `import bme280`, returning floats, tuples, lists and
//! dicts to Python (`HwFn::returning_values`, see the README, "Rich return
//! values").
//!
//!     cargo run --example bme280
//!
//! The driver only needs an `I2c` (one method, `write_read`). Here a
//! simulated chip answers, with the calibration and readings of the
//! datasheet's worked example (25.08 °C, 1006.53 hPa); on a board, implement
//! `I2c` with the HAL's I2C (see the README for esp-hal) and the same module
//! reads a real sensor.
//!
//! In Python:
//!
//!     import bme280
//!     t, p, h = bme280.read()        # (25.08, 1006.5325390625, 43.8564453125): °C, hPa, %RH
//!     bme280.read_dict()             # {'temperature': 25.08, 'pressure': ..., 'humidity': ...}
//!     bme280.info()                  # {'chip': 'BME280', 'id': 96, 'address': 118, 'calibration': {...}}
//!     bme280.scan()                  # [118]: the addresses where a BME280 answers
//!     bme280.raw()                   # (519888, 415148, 28000): the ADC values
//!     bme280.init(0x77)              # another address (OSError if nothing answers there)

use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Interpreter, Module, Output, Returns};

// ---- the driver -------------------------------------------------------------

/// No device acknowledged the address (nothing there, or a bus error).
#[derive(Debug)]
pub struct Nack;

/// What the driver needs from an I2C bus: write `out` (a register number),
/// then read `into` from the device at `addr`; or write `data` (a register
/// and its value).
pub trait I2c {
    fn write_read(&mut self, addr: u8, out: &[u8], into: &mut [u8]) -> Result<(), Nack>;
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack>;
}

pub const CHIP_ID: u8 = 0x60;
const REG_ID: u8 = 0xD0;
const REG_CALIB_A: u8 = 0x88; // T1..P9, H1: 26 bytes
const REG_CALIB_B: u8 = 0xE1; // H2..H6: 7 bytes
const REG_CTRL_HUM: u8 = 0xF2;
const REG_STATUS: u8 = 0xF3;
const REG_CTRL_MEAS: u8 = 0xF4;
const REG_DATA: u8 = 0xF7; // press[3], temp[3], hum[2]
/// ctrl_meas: temperature ×1 (bits 7-5), pressure ×1 (bits 4-2), forced
/// mode (bits 1-0): one measurement, then sleep
const CTRL_MEAS_FORCED: u8 = (0b001 << 5) | (0b001 << 2) | 0b01;

/// The factory calibration in the chip (datasheet section 4.2.2).
#[derive(Debug, Clone, Copy, Default)]
pub struct Calibration {
    pub t1: u16,
    pub t2: i16,
    pub t3: i16,
    pub p: [i32; 9], // P1 is unsigned 16-bit, P2..P9 signed 16-bit
    pub h1: u8,
    pub h2: i16,
    pub h3: u8,
    pub h4: i16,
    pub h5: i16,
    pub h6: i8,
}

impl Calibration {
    fn parse(a: &[u8; 26], b: &[u8; 7]) -> Self {
        let u16_at = |i: usize| u16::from_le_bytes([a[i], a[i + 1]]);
        let i16_at = |i: usize| i16::from_le_bytes([a[i], a[i + 1]]);
        let mut p = [0i32; 9];
        p[0] = u16_at(6) as i32;
        for (k, v) in p.iter_mut().enumerate().skip(1) {
            *v = i16_at(6 + 2 * k) as i32;
        }
        Self {
            t1: u16_at(0),
            t2: i16_at(2),
            t3: i16_at(4),
            p,
            h1: a[25],
            h2: i16::from_le_bytes([b[0], b[1]]),
            h3: b[2],
            // 12-bit values sharing the nibbles of 0xE5
            h4: ((b[3] as i8 as i16) << 4) | (b[4] & 0x0F) as i16,
            h5: ((b[5] as i8 as i16) << 4) | (b[4] >> 4) as i16,
            h6: b[6] as i8,
        }
    }
}

/// One measurement: the ADC values and what they mean (integer formulas of
/// the datasheet, section 4.2.3: exact on chips without an FPU too).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub adc_t: i32,
    pub adc_p: i32,
    pub adc_h: i32,
    /// °C × 100
    pub temperature: i32,
    /// Pa × 256
    pub pressure: u32,
    /// %RH × 1024
    pub humidity: u32,
}

pub struct Bme280 {
    pub address: u8,
    pub calibration: Calibration,
}

impl Bme280 {
    /// Checks the chip ID at `address` and reads the calibration.
    pub fn new(i2c: &mut impl I2c, address: u8) -> Result<Self, Error> {
        let mut id = [0u8];
        i2c.write_read(address, &[REG_ID], &mut id)
            .map_err(|_| Error::IoError)?;
        if id[0] != CHIP_ID {
            return Err(Error::IoError); // something else answers there
        }
        let (mut a, mut b) = ([0u8; 26], [0u8; 7]);
        i2c.write_read(address, &[REG_CALIB_A], &mut a)
            .map_err(|_| Error::IoError)?;
        i2c.write_read(address, &[REG_CALIB_B], &mut b)
            .map_err(|_| Error::IoError)?;
        Ok(Self {
            address,
            calibration: Calibration::parse(&a, &b),
        })
    }

    /// One measurement in forced mode (×1 oversampling: about 10 ms).
    pub fn read(&self, i2c: &mut impl I2c) -> Result<Reading, Error> {
        let io = |_| Error::IoError;
        // Humidity first: ctrl_hum takes effect with the write to ctrl_meas
        i2c.write(self.address, &[REG_CTRL_HUM, 0b001])
            .map_err(io)?;
        i2c.write(self.address, &[REG_CTRL_MEAS, CTRL_MEAS_FORCED])
            .map_err(io)?;
        let mut status = [0u8];
        for _ in 0..100 {
            i2c.write_read(self.address, &[REG_STATUS], &mut status)
                .map_err(io)?;
            if status[0] & 0x08 == 0 {
                break; // not measuring any more
            }
        }
        let mut d = [0u8; 8];
        i2c.write_read(self.address, &[REG_DATA], &mut d)
            .map_err(io)?;
        let adc_p = ((d[0] as i32) << 12) | ((d[1] as i32) << 4) | (d[2] as i32 >> 4);
        let adc_t = ((d[3] as i32) << 12) | ((d[4] as i32) << 4) | (d[5] as i32 >> 4);
        let adc_h = ((d[6] as i32) << 8) | d[7] as i32;
        let (t_fine, temperature) = self.temperature(adc_t);
        Ok(Reading {
            adc_t,
            adc_p,
            adc_h,
            temperature,
            pressure: self.pressure(adc_p, t_fine),
            humidity: self.humidity(adc_h, t_fine),
        })
    }

    /// (t_fine, °C × 100)
    fn temperature(&self, adc_t: i32) -> (i32, i32) {
        let c = &self.calibration;
        let (t1, t2, t3) = (c.t1 as i32, c.t2 as i32, c.t3 as i32);
        let var1 = (((adc_t >> 3) - (t1 << 1)) * t2) >> 11;
        let var2 = (((((adc_t >> 4) - t1) * ((adc_t >> 4) - t1)) >> 12) * t3) >> 14;
        let t_fine = var1 + var2;
        (t_fine, (t_fine * 5 + 128) >> 8)
    }

    /// Pa × 256 (the 64-bit formula)
    fn pressure(&self, adc_p: i32, t_fine: i32) -> u32 {
        let p = self.calibration.p.map(|v| v as i64);
        let mut var1 = t_fine as i64 - 128_000;
        let mut var2 = var1 * var1 * p[5];
        var2 += (var1 * p[4]) << 17;
        var2 += p[3] << 35;
        var1 = ((var1 * var1 * p[2]) >> 8) + ((var1 * p[1]) << 12);
        var1 = (((1i64 << 47) + var1) * p[0]) >> 33;
        if var1 == 0 {
            return 0; // no division by zero (a chip without calibration)
        }
        let mut pa = 1_048_576 - adc_p as i64;
        pa = (((pa << 31) - var2) * 3125) / var1;
        let var1 = (p[8] * (pa >> 13) * (pa >> 13)) >> 25;
        let var2 = (p[7] * pa) >> 19;
        (((pa + var1 + var2) >> 8) + (p[6] << 4)) as u32
    }

    /// %RH × 1024
    fn humidity(&self, adc_h: i32, t_fine: i32) -> u32 {
        let c = &self.calibration;
        let (h1, h2, h3) = (c.h1 as i32, c.h2 as i32, c.h3 as i32);
        let (h4, h5, h6) = (c.h4 as i32, c.h5 as i32, c.h6 as i32);
        let mut v = t_fine - 76_800;
        v = ((((adc_h << 14) - (h4 << 20) - (h5 * v)) + 16_384) >> 15)
            * (((((((v * h6) >> 10) * (((v * h3) >> 11) + 32_768)) >> 10) + 2_097_152) * h2
                + 8192)
                >> 14);
        v -= ((((v >> 15) * (v >> 15)) >> 7) * h1) >> 4;
        (v.clamp(0, 419_430_400) >> 12) as u32
    }
}

// ---- the module -------------------------------------------------------------

const BME280_MODULE: Module = Module {
    name: "bme280",
    doc: "Bosch BME280: temperature, pressure, humidity (I2C)",
    functions: &[
        HwFn::with_args(
            "init",
            0,
            1,
            "[address]",
            "Find the sensor (0x76 by default)",
        ),
        HwFn::new("read", "", "(°C, hPa, %RH) as floats").returning_values(),
        HwFn::new("read_dict", "", "{'temperature', 'pressure', 'humidity'}").returning_values(),
        HwFn::new("raw", "", "(adc_T, adc_P, adc_H): the ADC values").returning_values(),
        HwFn::new("info", "", "Chip, address and calibration as a dict").returning_values(),
        HwFn::new("scan", "", "Addresses where a BME280 answers: [0x76, 0x77]").returning_values(),
    ],
    constants: &[
        ("ADDRESS", 0x76),
        ("ADDRESS_ALT", 0x77),
        ("CHIP_ID", CHIP_ID as i64),
    ],
};

/// A board with a BME280 module: the bus and the sensor once found.
pub struct Board<B: I2c> {
    pub i2c: B,
    pub sensor: Option<Bme280>,
}

impl<B: I2c> Board<B> {
    pub fn new(i2c: B) -> Self {
        Self { i2c, sensor: None }
    }

    /// The sensor, found at 0x76 on first use (`bme280.init()` picks another
    /// address).
    fn sensor(&mut self) -> Result<&Bme280, Error> {
        if self.sensor.is_none() {
            self.sensor = Some(Bme280::new(&mut self.i2c, 0x76)?);
        }
        Ok(self.sensor.as_ref().unwrap())
    }

    fn reading(&mut self) -> Result<Reading, Error> {
        self.sensor()?;
        let sensor = self.sensor.as_ref().unwrap();
        sensor.read(&mut self.i2c)
    }
}

impl<B: I2c> HardwareBridge for Board<B> {
    fn modules(&self) -> &[Module] {
        &[BME280_MODULE]
    }

    // Functions that return an int or nothing
    fn call_module(
        &mut self,
        module: &str,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        match (module, name) {
            ("bme280", "init") => {
                let address = args.first().map_or(Ok(0x76), |a| a.int())?;
                let address = u8::try_from(address).map_err(|_| Error::ValueError)?;
                self.sensor = None;
                self.sensor = Some(Bme280::new(&mut self.i2c, address)?);
                Ok(None)
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }

    // Functions marked `returning_values`: the values go into `out`
    fn call_values(
        &mut self,
        module: Option<&str>,
        name: &str,
        _args: &[Arg<'_>],
        out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        match (module, name) {
            (Some("bme280"), "read") => {
                let r = self.reading()?;
                out.begin_tuple()?;
                out.float(r.temperature as f64 / 100.0)?;
                out.float(r.pressure as f64 / 25_600.0)?; // Pa × 256 -> hPa
                out.float(r.humidity as f64 / 1024.0)?;
                out.end()
            }
            (Some("bme280"), "read_dict") => {
                let r = self.reading()?;
                out.begin_dict()?;
                out.str("temperature")?;
                out.float(r.temperature as f64 / 100.0)?;
                out.str("pressure")?;
                out.float(r.pressure as f64 / 25_600.0)?;
                out.str("humidity")?;
                out.float(r.humidity as f64 / 1024.0)?;
                out.end()
            }
            (Some("bme280"), "raw") => {
                let r = self.reading()?;
                out.begin_tuple()?;
                for v in [r.adc_t, r.adc_p, r.adc_h] {
                    out.int(v as i64)?;
                }
                out.end()
            }
            (Some("bme280"), "info") => {
                let s = self.sensor()?;
                let (address, c) = (s.address, s.calibration);
                out.begin_dict()?;
                out.str("chip")?;
                out.str("BME280")?;
                out.str("id")?;
                out.int(CHIP_ID as i64)?;
                out.str("address")?;
                out.int(address as i64)?;
                // A dict inside the dict
                out.str("calibration")?;
                out.begin_dict()?;
                let names = ["T1", "T2", "T3"];
                for (key, v) in names.iter().zip([c.t1 as i64, c.t2 as i64, c.t3 as i64]) {
                    out.str(key)?;
                    out.int(v)?;
                }
                for (k, v) in c.p.iter().enumerate() {
                    let key = ["P1", "P2", "P3", "P4", "P5", "P6", "P7", "P8", "P9"][k];
                    out.str(key)?;
                    out.int(*v as i64)?;
                }
                let humidity = [
                    c.h1 as i64,
                    c.h2 as i64,
                    c.h3 as i64,
                    c.h4 as i64,
                    c.h5 as i64,
                    c.h6 as i64,
                ];
                for (key, v) in ["H1", "H2", "H3", "H4", "H5", "H6"].iter().zip(humidity) {
                    out.str(key)?;
                    out.int(v)?;
                }
                out.end()?;
                out.end()
            }
            (Some("bme280"), "scan") => {
                out.begin_list()?;
                for address in [0x76, 0x77] {
                    let mut id = [0u8];
                    let found = self.i2c.write_read(address, &[REG_ID], &mut id).is_ok();
                    if found && id[0] == CHIP_ID {
                        out.int(address as i64)?;
                    }
                }
                out.end()
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }
}

// ---- a simulated chip (instead of real hardware) ----------------------------

/// A BME280 at one address: its registers, with the datasheet example's
/// calibration (section 8.1 of the BMP280 datasheet, the same formulas for
/// temperature and pressure) and typical humidity values.
pub struct SimulatedBme280 {
    pub address: u8,
    pub regs: [u8; 256],
}

impl SimulatedBme280 {
    pub fn new(address: u8) -> Self {
        let mut regs = [0u8; 256];
        regs[REG_ID as usize] = CHIP_ID;
        let mut a = [0u8; 26];
        let words: [i32; 12] = [
            27504, 26435, -1000, // T1..T3
            36477, -10685, 3024, 2855, 140, -7, 15500, -14600, 6000, // P1..P9
        ];
        for (i, w) in words.iter().enumerate() {
            a[2 * i..2 * i + 2].copy_from_slice(&(*w as u16).to_le_bytes());
        }
        a[25] = 75; // H1
        regs[REG_CALIB_A as usize..REG_CALIB_A as usize + 26].copy_from_slice(&a);
        // H2 = 362, H3 = 0, H4 = 313, H5 = 50, H6 = 30 (the 12-bit H4/H5 share 0xE5)
        let (h4, h5) = (313i16, 50i16);
        let b = [
            (362u16 & 0xFF) as u8,
            (362u16 >> 8) as u8,
            0,
            (h4 >> 4) as u8,
            ((h5 as u8 & 0x0F) << 4) | (h4 as u8 & 0x0F),
            (h5 >> 4) as u8,
            30,
        ];
        regs[REG_CALIB_B as usize..REG_CALIB_B as usize + 7].copy_from_slice(&b);
        let mut chip = Self { address, regs };
        chip.set_adc(519_888, 415_148, 28_000);
        chip
    }

    /// The values the next measurement reads.
    pub fn set_adc(&mut self, adc_t: u32, adc_p: u32, adc_h: u16) {
        let at = REG_DATA as usize;
        let three = |v: u32| [(v >> 12) as u8, (v >> 4) as u8, ((v & 0xF) << 4) as u8];
        self.regs[at..at + 3].copy_from_slice(&three(adc_p));
        self.regs[at + 3..at + 6].copy_from_slice(&three(adc_t));
        self.regs[at + 6..at + 8].copy_from_slice(&adc_h.to_be_bytes());
    }
}

impl I2c for SimulatedBme280 {
    fn write_read(&mut self, addr: u8, out: &[u8], into: &mut [u8]) -> Result<(), Nack> {
        if addr != self.address {
            return Err(Nack); // no ACK: nothing at that address
        }
        let reg = out[0] as usize;
        into.copy_from_slice(&self.regs[reg..reg + into.len()]);
        Ok(())
    }

    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack> {
        if addr != self.address {
            return Err(Nack);
        }
        self.regs[data[0] as usize] = data[1];
        Ok(())
    }
}

// ---- running a script -------------------------------------------------------

struct Text(String);

impl Output for Text {
    fn write_str(&mut self, s: &str) {
        self.0.push_str(s);
    }
}

/// Runs `script` with a board whose BME280 is `chip`: (output, error text).
pub fn run(script: &str, chip: SimulatedBme280) -> (String, String) {
    let mut out = Text(String::new());
    let mut board = Board::new(chip);
    let mut heap = vec![0u8; 16 * 1024];
    let mut vm = Interpreter::with_hardware(&mut out, &mut board).with_heap(&mut heap);
    let mut error = Text(String::new());
    if let Err(err) = vm.eval(script) {
        vm.write_exception(err, &mut error);
    }
    let _ = vm;
    (out.0, error.0)
}

pub const SCRIPT: &str = "\
import bme280
from math import log
print(bme280.info()['chip'], 'at', hex(bme280.ADDRESS), 'found at', bme280.scan())
t, p, h = bme280.read()
print('%.2f C  %.2f hPa  %.1f %%RH' % (t, p, h))
# The dew point (Magnus formula), in Python on the Rust values
g = log(h / 100) + 17.62 * t / (243.12 + t)
print('dew point %.1f C' % (243.12 * g / (17.62 - g)))
d = bme280.read_dict()
print(sorted(d), d['pressure'] > 1000)
print('raw', bme280.raw(), 'T1', bme280.info()['calibration']['T1'])
try:
    bme280.init(bme280.ADDRESS_ALT)
except OSError:
    print('no sensor at', hex(bme280.ADDRESS_ALT))
";

#[allow(dead_code)]
fn main() {
    let (out, error) = run(SCRIPT, SimulatedBme280::new(0x76));
    print!("{}", out);
    if !error.is_empty() {
        eprintln!("{}", error);
        std::process::exit(1);
    }
}
