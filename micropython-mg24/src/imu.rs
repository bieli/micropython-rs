//! The XIAO MG24 Sense's motion sensor, an ST LSM6DS3TR-C (accelerometer and
//! gyroscope), as the native module `imu`:
//!
//!     import imu
//!     ax, ay, az, gx, gy, gz = imu.read()   # g and degrees per second
//!     imu.accel(), imu.gyro()               # (x, y, z) of one of them
//!     imu.temperature()                     # the sensor's die, degrees C
//!
//! It's on I2C0, SDA PB2 and SCL PB3 (the D13/D14 pads), powered from PD5,
//! at address 0x6A (from the board's schematic). Set up on first use: power
//! on, I2C at 400 kHz, WHO_AM_I checked (0x6A), both at 104 Hz, +-2 g and
//! 250 dps, with block data update (a reading never mixes two samples).
//! The values are the datasheet's sensitivities: 0.061 mg, 8.75 mdps and
//! 1/256 degrees C (from 25) per count.

use micropython_rs::{Error, HwFn, Module, Returns};

use crate::gpio::{self, PinId};
use crate::i2c::I2c;

const ADDR: u8 = 0x6A;
const WHO_AM_I: u8 = 0x0F;
const CTRL1_XL: u8 = 0x10;
const CTRL2_G: u8 = 0x11;
const CTRL3_C: u8 = 0x12;
const OUT_TEMP_L: u8 = 0x20;
const OUTX_L_G: u8 = 0x22;
const OUTX_L_XL: u8 = 0x28;

const G_PER_COUNT: f64 = 0.061e-3;
const DPS_PER_COUNT: f64 = 8.75e-3;

/// PD5 (the sensor's power), PB2 (SDA), PB3 (SCL).
const POWER_PIN: i64 = 3 * 16 + 5;
const SDA_PIN: i64 = 16 + 2;
const SCL_PIN: i64 = 16 + 3;

pub const MODULE: Module = Module {
    name: "imu",
    doc: "The Sense's motion sensor (LSM6DS3TR-C): g, degrees per second, degrees C",
    functions: &[
        HwFn::new(
            "read",
            "",
            "(ax, ay, az, gx, gy, gz): g and degrees per second",
        )
        .returning_values(),
        HwFn::new("accel", "", "(x, y, z) in g (+-2 g)").returning_values(),
        HwFn::new("gyro", "", "(x, y, z) in degrees per second (+-250)").returning_values(),
        HwFn::new("temperature", "", "The sensor's temperature, degrees C").returning_values(),
    ],
    constants: &[("ADDRESS", ADDR as i64)],
};

pub struct Imu {
    bus: I2c,
}

impl Imu {
    /// Powers the sensor and sets it up; `Err(IoError)` if it doesn't answer.
    pub fn new(pclk_hz: u32) -> Result<Self, Error> {
        gpio::pin_mode(POWER_PIN, gpio::OUT)?;
        gpio::write(PinId::new(POWER_PIN)?, true);
        crate::clock::delay_us(20_000); // the datasheet's boot time: 15 ms
        let mut bus = I2c::init(PinId::new(SDA_PIN)?, PinId::new(SCL_PIN)?, 400_000, pclk_hz)?;
        let mut id = [0u8];
        bus.write_read(ADDR, &[WHO_AM_I], &mut id)?;
        if id[0] != 0x6A {
            return Err(Error::IoError);
        }
        bus.write(ADDR, &[CTRL3_C, 0x44])?; // BDU, register address auto-increment
        bus.write(ADDR, &[CTRL1_XL, 0x40])?; // accelerometer 104 Hz, +-2 g
        bus.write(ADDR, &[CTRL2_G, 0x40])?; // gyroscope 104 Hz, 250 dps
                                            // The gyroscope needs longer than the accelerometer to settle: its
                                            // first samples after power-on were off by tens of dps (measured)
        crate::clock::delay_us(100_000);
        Ok(Self { bus })
    }

    /// Three signed 16-bit values from `reg` on.
    fn xyz(&mut self, reg: u8) -> Result<[i16; 3], Error> {
        let mut b = [0u8; 6];
        self.bus.write_read(ADDR, &[reg], &mut b)?;
        Ok([0, 1, 2].map(|i| i16::from_le_bytes([b[2 * i], b[2 * i + 1]])))
    }

    pub fn call(&mut self, name: &str, out: &mut Returns<'_>) -> Result<(), Error> {
        match name {
            "read" => {
                let a = self.xyz(OUTX_L_XL)?;
                let g = self.xyz(OUTX_L_G)?;
                out.begin_tuple()?;
                for v in a {
                    out.float(v as f64 * G_PER_COUNT)?;
                }
                for v in g {
                    out.float(v as f64 * DPS_PER_COUNT)?;
                }
                out.end()
            }
            "accel" | "gyro" => {
                let (reg, scale) = if name == "accel" {
                    (OUTX_L_XL, G_PER_COUNT)
                } else {
                    (OUTX_L_G, DPS_PER_COUNT)
                };
                out.begin_tuple()?;
                for v in self.xyz(reg)? {
                    out.float(v as f64 * scale)?;
                }
                out.end()
            }
            "temperature" => {
                let mut b = [0u8; 2];
                self.bus.write_read(ADDR, &[OUT_TEMP_L], &mut b)?;
                out.float(i16::from_le_bytes(b) as f64 / 256.0 + 25.0)
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }
}
