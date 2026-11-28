//! micropython-rs firmware for nRF52840 boards with the Adafruit UF2
//! bootloader: the nice!nano v2 and its SuperMini clones (Nordic nRF52840,
//! Arm Cortex-M4F at 64 MHz, 1 MB flash, 256 KB RAM). Flashed by copying a
//! UF2 file to the bootloader's USB drive (`make flash`); see memory/memory.x
//! for the flash layout.
//!
//! Step 1: the board runs our code. The blue LED (P0.15, lit when high)
//! flashes twice a second, unlike the factory firmware's slow pulse.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use nrf52840_pac as pac;

/// Where this firmware's vector table is (memory/memory.x: FLASH).
const APP_START: u32 = 0x0002_6000;
const LED_PIN: usize = 15;
/// The CPU's clock at reset: HFINT, 64 MHz.
const CPU_HZ: u32 = 64_000_000;

#[entry]
fn main() -> ! {
    // SAFETY: our vector table, before any interrupt is enabled: the MBR and
    // the SoftDevice would forward them here anyway, this skips the detour
    unsafe { (*cortex_m::peripheral::SCB::PTR).vtor.write(APP_START) };
    let p = pac::Peripherals::take().unwrap();
    p.P0.pin_cnf[LED_PIN].write(|w| w.dir().output().input().disconnect());
    // SAFETY: OUTSET/OUTCLR take any bit mask; only the LED's bit is set
    let led = |on: bool| unsafe {
        if on {
            p.P0.outset.write(|w| w.bits(1 << LED_PIN));
        } else {
            p.P0.outclr.write(|w| w.bits(1 << LED_PIN));
        }
    };
    let ms = |n: u32| cortex_m::asm::delay(CPU_HZ / 1000 * n);
    loop {
        for _ in 0..2 {
            led(true);
            ms(60);
            led(false);
            ms(120);
        }
        ms(640);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    cortex_m::peripheral::SCB::sys_reset()
}
