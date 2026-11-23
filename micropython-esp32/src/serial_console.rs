use esp_println::print;
use micropython_rs::Output;

/// Connects Python's `print()` directly to the ESP32 UART/USB serial monitor.
/// Works on every ESP32 board, no extra hardware required.
pub struct EspSerialOut;

impl Output for EspSerialOut {
    fn write_str(&mut self, s: &str) {
        write_serial(s);
    }
}

/// Prints to the serial port with `\r\n` line endings, which terminals
/// and MicroPython tools (`mpremote`) expect.
pub fn write_serial(s: &str) {
    let mut lines = s.split('\n');
    if let Some(first) = lines.next() {
        print!("{}", first);
    }
    for line in lines {
        print!("\r\n{}", line);
    }
}
