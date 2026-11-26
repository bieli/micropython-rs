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
//!
//! ESP32-C3 (RISC-V): GPIO0-21; ADC1 is on GPIO0-4, flash on 11-17, USB on
//! 18/19. The choices follow the ESP32-C3 Super Mini: GPIO0, 1 and 3 are the
//! ADC inputs, 6/7 the I2C bus (8 is its LED, 9 its BOOT button).

#[cfg(any(
    all(feature = "esp32", feature = "esp32s3"),
    all(feature = "esp32", feature = "esp32c3"),
    all(feature = "esp32s3", feature = "esp32c3")
))]
compile_error!("choose one chip: `esp32`, `esp32s3` or `esp32c3` (make flash CHIP=...)");
#[cfg(not(any(feature = "esp32", feature = "esp32s3", feature = "esp32c3")))]
compile_error!("choose a chip: `esp32`, `esp32s3` or `esp32c3` (make flash CHIP=...)");
#[cfg(all(not(feature = "esp32"), feature = "tft"))]
compile_error!("the `tft` build is for the TTGO T-Display, an ESP32 board");
#[cfg(all(feature = "esp32c3", cpu_mhz = "240"))]
compile_error!("the ESP32-C3 runs at 80 or 160 MHz (make flash CHIP=esp32c3 CPU_MHZ=160)");

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

#[cfg(feature = "esp32c3")]
mod pins {
    pub use esp_hal::peripherals::{GPIO0 as AdcA, GPIO1 as AdcB, GPIO3 as AdcC};
    pub use esp_hal::peripherals::{GPIO6 as Sda, GPIO7 as Scl};

    pub const NAME: &str = "ESP32-C3";
    pub const GPIO_COUNT: usize = 22;
    pub const ADC_PINS: [u8; 3] = [0, 1, 3];
    pub const I2C_SDA: u8 = 6;
    pub const I2C_SCL: u8 = 7;

    pub const fn gpio_exists(pin: u8) -> bool {
        pin <= 21
    }

    pub const fn can_output(_pin: u8) -> bool {
        true
    }
}

pub use pins::*;

/// For `lscpu`: the CPU core, cores, top clock, FPU, radios, internal SRAM.
#[cfg(feature = "esp32")]
pub mod about {
    pub const ARCH: &str = "xtensa";
    pub const CORE: &str = "Xtensa LX6";
    pub const CORES: u32 = 2;
    pub const MAX_MHZ: u32 = 240;
    pub const FLAGS: &str = "fpu wifi bt ble";
    pub const SRAM: u32 = 520 * 1024;
}
#[cfg(feature = "esp32s3")]
pub mod about {
    pub const ARCH: &str = "xtensa";
    pub const CORE: &str = "Xtensa LX7";
    pub const CORES: u32 = 2;
    pub const MAX_MHZ: u32 = 240;
    pub const FLAGS: &str = "fpu wifi ble usb-jtag";
    pub const SRAM: u32 = 512 * 1024;
}
#[cfg(feature = "esp32c3")]
pub mod about {
    pub const ARCH: &str = "riscv32";
    pub const CORE: &str = "RISC-V RV32IMC";
    pub const CORES: u32 = 1;
    pub const MAX_MHZ: u32 = 160;
    pub const FLAGS: &str = "wifi ble usb-jtag";
    pub const SRAM: u32 = 400 * 1024;
}

/// The board the pins are chosen for.
pub const BOARD: &str = if cfg!(feature = "tft") {
    "LilyGO TTGO T-Display"
} else if cfg!(feature = "esp32s3") {
    "Seeed XIAO ESP32-S3 (or another ESP32-S3)"
} else if cfg!(feature = "esp32c3") {
    "ESP32-C3 Super Mini (or another ESP32-C3)"
} else {
    "ESP32 DevKit (generic)"
};

/// How the ADC readings are calibrated. ESP32: none in esp-hal (raw values,
/// converted with the eFuse reference voltage, see `board::raw_to_mv`).
/// ESP32-S3 and C3: esp-hal's curve fitting with the chip's eFuse
/// calibration, so a reading is already in millivolts. (The C3's full scale
/// at 11 dB is about 2.5 V, the S3's 3.1 V.)
#[cfg(feature = "esp32")]
pub type AdcCal<'d> = ();
#[cfg(any(feature = "esp32s3", feature = "esp32c3"))]
pub type AdcCal<'d> = esp_hal::analog::adc::AdcCalCurve<esp_hal::peripherals::ADC1<'d>>;

/// Readings of `AdcCal` pins are millivolts (not raw 12-bit values).
pub const ADC_READS_MV: bool = cfg!(any(feature = "esp32s3", feature = "esp32c3"));

/// The chip's console is its own USB port (USB-Serial/JTAG), not UART0.
pub const USB_CONSOLE: bool = cfg!(any(feature = "esp32s3", feature = "esp32c3"));

/// Full scale at 11 dB attenuation, mV: what a raw 4095 means when the
/// calibrated chip's millivolts are turned back into a raw value.
pub const ADC_FULL_SCALE_MV: i64 = if cfg!(feature = "esp32c3") {
    2500
} else {
    3100
};
