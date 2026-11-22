#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::{
    delay::Delay,
    gpio::{Level, Output as GpioOutput, OutputConfig},
    main,
};
use esp_println::{print, println};
use micropython_rs::{HardwareBridge, Interpreter, Output};

esp_bootloader_esp_idf::esp_app_desc!();

/// Connects Python's `print()` directly to the ESP32 UART/USB serial monitor.
struct EspSerialOut;

impl Output for EspSerialOut {
    fn write_str(&mut self, s: &str) {
        print!("{}", s);
    }
}

/// Exposes ESP32 hardware peripherals (GPIO LED + blocking Delay) to Python scripts.
struct Esp32Board<'d> {
    led: GpioOutput<'d>,
    delay: Delay,
}

impl<'d> HardwareBridge for Esp32Board<'d> {
    fn call(&mut self, name: &str, args: &[i64]) -> Result<Option<i64>, ()> {
        match (name, args) {
            ("led_on", []) => {
                self.led.set_high();
                Ok(None)
            }
            ("led_off", []) => {
                self.led.set_low();
                Ok(None)
            }
            ("delay_ms", [ms]) if *ms >= 0 => {
                self.delay.delay_millis(*ms as u32);
                Ok(None)
            }
            _ => Err(()),
        }
    }
}

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Most ESP32 DevKit boards wire the onboard blue LED to GPIO2.
    // (If your board uses another pin, e.g., GPIO5 or GPIO4, swap `peripherals.GPIO2` below.)

    //let led = GpioOutput::new(peripherals.GPIO2, Level::Low, OutputConfig::default());

    // LilyGO TTGO T-Display: GPIO4 display backlight TFT
    let led = GpioOutput::new(peripherals.GPIO4, Level::Low, OutputConfig::default());
    let delay = Delay::new();

    println!("\n=== micropython-rs starting on ESP32 ===");

    let mut serial = EspSerialOut;
    let mut board = Esp32Board { led, delay };

    let script = r#"
# 1. Interpreter variables, reassignment, and print test
chip = 'ESP32'
a = 21
b = 21
msg = 'Hello from'
print(msg, chip)
print('a =', a, 'b =', b)

# 2. Hardware GPIO LED + delay_ms test
short_wait = 250
long_wait = 600

print('Blinking onboard LED on GPIO4...')
led_on()
delay_ms(short_wait)
led_off()
delay_ms(short_wait)

led_on()
delay_ms(long_wait)
led_off()
delay_ms(short_wait)

# Leave LED turned on at the end
led_on()
print('LED sequence complete!')
"#;

    let mut vm = Interpreter::with_hardware(&mut serial, &mut board);

    match vm.eval(script) {
        Ok(()) => println!("=== Execution complete ==="),
        Err(err) => println!("Python Runtime Error: {:?}", err),
    }

    loop {}
}
