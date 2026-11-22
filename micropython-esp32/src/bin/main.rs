#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::main;
use esp_println::{print, println};
use micropython_rs::{Interpreter, Output};

esp_bootloader_esp_idf::esp_app_desc!();

struct EspSerialOut;

impl Output for EspSerialOut {
    fn write_str(&mut self, s: &str) {
        print!("{}", s);
    }
}

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    println!("\n=== micropython-rs running on ESP32 ===");

    let script = r#"
chip = 'ESP32'
a = 21
b = 21
print('Hello from', chip)
print('a =', a, 'b =', b)
"#;

    let mut serial = EspSerialOut;
    let mut vm = Interpreter::new(&mut serial);

    match vm.eval(script) {
        Ok(()) => println!("=== Execution complete ==="),
        Err(err) => println!("Python Runtime Error: {:?}", err),
    }

    loop {}
}
