use esp_println::print;
use micropython_rs::Output;

/// Connects Python's `print()` directly to the ESP32 UART/USB serial monitor.
/// Works on every ESP32 board, no extra hardware required.
pub struct EspSerialOut;

impl Output for EspSerialOut {
    fn write_str(&mut self, s: &str) {
        print!("{}", s);
    }
}
