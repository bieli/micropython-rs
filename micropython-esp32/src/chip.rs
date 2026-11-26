//! What differs between the chips the firmware runs on (`esp32` or
//! `esp32s3` feature, `make flash CHIP=...`): GPIO numbers, the pins wired to
//! the ADC, the default I2C pins, which pins can be outputs.
//!
//! ESP32: GPIO0-39; GPIO34-39 are input-only, and the ADC uses three of them
//! (34, 36, 39), so analog inputs never collide with outputs.
//!
//! ESP32-S3: GPIO0-21 and 26-48, all of them outputs; ADC1 is on GPIO1-10.
//! The pin choices follow the Seeed XIAO ESP32-S3: A0-A2 (GPIO1-3) are the
//! ADC inputs, D4/D5 (GPIO5/6) the I2C bus, the user LED is GPIO21.

#[cfg(all(feature = "esp32", feature = "esp32s3"))]
compile_error!("choose one chip: the `esp32` or the `esp32s3` feature (make flash CHIP=...)");
#[cfg(not(any(feature = "esp32", feature = "esp32s3")))]
compile_error!("choose a chip: the `esp32` or the `esp32s3` feature (make flash CHIP=...)");
#[cfg(all(feature = "esp32s3", feature = "tft"))]
compile_error!("the `tft` build is for the TTGO T-Display, an ESP32 board");

#[cfg(feature = "esp32")]
mod pins {
    pub use esp_hal::peripherals::{GPIO21 as Sda, GPIO22 as Scl};
    pub use esp_hal::peripherals::{GPIO34 as AdcA, GPIO36 as AdcB, GPIO39 as AdcC};

    /// The chip's name, for messages.
    pub const NAME: &str = "ESP32";
    /// GPIO numbers are below this.
    pub const GPIO_COUNT: usize = 40;
    /// The pins of `AdcA`, `AdcB`, `AdcC`.
    pub const ADC_PINS: [u8; 3] = [34, 36, 39];
    pub const I2C_SDA: u8 = 21;
    pub const I2C_SCL: u8 = 22;

    /// The pin exists (GPIO20, 24 and 28-31 aren't bonded out).
    pub const fn gpio_exists(pin: u8) -> bool {
        matches!(pin, 0..=19 | 21..=23 | 25..=27 | 32..=39)
    }

    /// The pin can drive an output (GPIO34-39 are input-only).
    pub const fn can_output(pin: u8) -> bool {
        pin < 34
    }
}

#[cfg(feature = "esp32s3")]
mod pins {
    pub use esp_hal::peripherals::{GPIO1 as AdcA, GPIO2 as AdcB, GPIO3 as AdcC};
    pub use esp_hal::peripherals::{GPIO5 as Sda, GPIO6 as Scl};

    pub const NAME: &str = "ESP32-S3";
    pub const GPIO_COUNT: usize = 49;
    pub const ADC_PINS: [u8; 3] = [1, 2, 3];
    pub const I2C_SDA: u8 = 5;
    pub const I2C_SCL: u8 = 6;

    pub const fn gpio_exists(pin: u8) -> bool {
        matches!(pin, 0..=21 | 26..=48)
    }

    pub const fn can_output(_pin: u8) -> bool {
        true
    }
}

pub use pins::*;

/// How the ADC readings are calibrated. ESP32: none in esp-hal (raw values,
/// converted with the eFuse reference voltage, see `board::raw_to_mv`).
/// ESP32-S3: esp-hal's curve fitting with the chip's eFuse calibration, so
/// a reading is already in millivolts.
#[cfg(feature = "esp32")]
pub type AdcCal<'d> = ();
#[cfg(feature = "esp32s3")]
pub type AdcCal<'d> = esp_hal::analog::adc::AdcCalCurve<esp_hal::peripherals::ADC1<'d>>;

/// Readings of `AdcCal` pins are millivolts (not raw 12-bit values).
pub const ADC_READS_MV: bool = cfg!(feature = "esp32s3");

/// Full scale at 11 dB attenuation, mV: what a raw 4095 means when the
/// calibrated chip's millivolts are turned back into a raw value.
pub const ADC_FULL_SCALE_MV: i64 = 3100;
