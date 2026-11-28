//! The chip's clocks, set once at boot before anything uses them.
//!
//! The XIAO MG24 has a 39 MHz crystal (for the radio). It becomes the clock
//! of the timers (EM01GRPACLK: the microsecond counter, PWM) and of the
//! EUSARTs (EM01GRPCCLK: UART, SPI), so time and baud rates are as accurate
//! as the crystal, not the RC oscillator. The CPU runs at 78 MHz, the chip's
//! maximum: DPLL0 locks HFRCODPLL (in its 80 MHz band) to the crystal at
//! 39 MHz * 4000 / 2000, and PCLK is HCLK / 2 (39 MHz: its limit is 50).
//! The flash's reset setting (2 wait states) and core voltage (VSCALE2) are
//! enough for 78 MHz. If the DPLL doesn't lock, the CPU runs from the crystal
//! at 39 MHz; if the crystal doesn't start, everything stays on HFRCODPLL's
//! 38 MHz band (mg24-hal sets it up).
//!
//! The drivers take their frequencies from here (`cpu_hz`, `pclk_hz`,
//! `periph_hz`), not from mg24-hal's `sysclk_hz`, which only knows HFRCODPLL.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use efr32mg24_pac as pac;

const HFXO_HZ: u32 = 39_000_000;
/// HFXO0_LOCK's unlock key.
const HFXO_UNLOCK: u32 = 22542;
/// The crystal's tuning capacitance (XI and XO): Silicon Labs' usual value
/// for 39 MHz radio boards. A different one only shifts the frequency by
/// some ppm.
const CTUNE: u32 = 120;

static CPU_HZ: AtomicU32 = AtomicU32::new(38_000_000);
static PCLK_HZ: AtomicU32 = AtomicU32::new(38_000_000);
static PERIPH_HZ: AtomicU32 = AtomicU32::new(38_000_000);
static CRYSTAL: AtomicBool = AtomicBool::new(false);

/// The CPU's clock (HCLK), in Hz.
pub fn cpu_hz() -> u32 {
    CPU_HZ.load(Ordering::Relaxed)
}

/// PCLK (USART0, the console; I2C1; I2C0 has half of it), in Hz.
pub fn pclk_hz() -> u32 {
    PCLK_HZ.load(Ordering::Relaxed)
}

/// EM01GRPACLK and EM01GRPCCLK (TIMER0-4, EUSART0-1), in Hz.
pub fn periph_hz() -> u32 {
    PERIPH_HZ.load(Ordering::Relaxed)
}

/// Whether the clocks run from the crystal (`lscpu`).
pub fn crystal() -> bool {
    CRYSTAL.load(Ordering::Relaxed)
}

/// Starts the crystal and moves the clocks to it; `false` (clocks left on
/// HFRCODPLL at `hfrco_hz`) if it isn't ready within 10 ms.
pub fn init(hfrco_hz: u32) -> bool {
    CPU_HZ.store(hfrco_hz, Ordering::Relaxed);
    PCLK_HZ.store(hfrco_hz, Ordering::Relaxed);
    PERIPH_HZ.store(hfrco_hz, Ordering::Relaxed);
    // SAFETY: CMU and HFXO0, at boot, before any peripheral is clocked from
    // them; HFXO's configuration can only change while it's off (it is at reset)
    unsafe {
        let cmu = &*pac::CmuS::ptr();
        cmu.clken0().modify(|_, w| w.hfxo0().set_bit());
        let hfxo = &*pac::Hfxo0S::ptr();
        hfxo.lock().write(|w| w.bits(HFXO_UNLOCK));
        // Core bias 60 (the reset value), CTUNE on XI and XO, both fixed
        // capacitors (CTUNEFIXANA = 3)
        hfxo.xtalctrl()
            .write(|w| w.bits(60 | CTUNE << 8 | CTUNE << 16 | 3 << 24));
        // Crystal mode; force it on (no on-demand), tuning not forced to max
        hfxo.cfg().write(|w| w.bits(0x1000_0000));
        hfxo.ctrl().write(|w| w.bits(1 << 16));
        // Ready (bit 0) and its core bias optimized (bit 1); the RC
        // oscillator still runs the CPU, so a spin count is the timeout
        // (10 ms at 38 MHz is well over 100 000 of these loops)
        let mut ready = false;
        for _ in 0..400_000 {
            if hfxo.status().read().bits() & 0b11 == 0b11 {
                ready = true;
                break;
            }
        }
        if !ready {
            hfxo.ctrl().write(|w| w.bits(0));
            return false;
        }
        // Timers and EUSARTs, then the CPU (and with it PCLK)
        cmu.em01grpaclkctrl().write(|w| w.bits(2));
        cmu.em01grpcclkctrl().write(|w| w.bits(2));
        cmu.sysclkctrl()
            .modify(|r, w| w.bits(r.bits() & !0b111 | 3));
    }
    CPU_HZ.store(HFXO_HZ, Ordering::Relaxed);
    PCLK_HZ.store(HFXO_HZ, Ordering::Relaxed);
    PERIPH_HZ.store(HFXO_HZ, Ordering::Relaxed);
    CRYSTAL.store(true, Ordering::Relaxed);
    if dpll_78mhz() {
        CPU_HZ.store(2 * HFXO_HZ, Ordering::Relaxed);
    }
    true
}

/// HFRCODPLL's factory trims in DEVINFO (18 words, one per band).
fn hfrco_cal(i: usize) -> u32 {
    // SAFETY: DEVINFO is read-only factory data; the trims are 18
    // consecutive words from HFRCODPLLCAL0
    unsafe {
        let devinfo = &*pac::Devinfo::ptr();
        (devinfo.hfrcodpllcal0() as *const _ as *const u32)
            .add(i)
            .read_volatile()
    }
}

/// The DEVINFO trim for HFRCODPLL's 80 MHz band: the one with the highest
/// frequency range (bits 16-20) and no output divider (bits 24-25).
pub fn cal_80mhz_index() -> usize {
    (0..18)
        .filter(|&i| (hfrco_cal(i) >> 24) & 0b11 == 0)
        .max_by_key(|&i| (hfrco_cal(i) >> 16) & 0x1F)
        .unwrap_or(15)
}

/// Locks HFRCODPLL to the crystal at 78 MHz and moves the CPU to it, PCLK
/// halved first; `false` (the CPU left on the crystal) if it doesn't lock.
fn dpll_78mhz() -> bool {
    let cal = hfrco_cal(cal_80mhz_index());
    // SAFETY: CMU, HFRCO0 and DPLL0 at boot; nothing runs from HFRCODPLL
    // any more (the timers and EUSARTs are on the crystal)
    unsafe {
        let cmu = &*pac::CmuS::ptr();
        cmu.clken0()
            .modify(|_, w| w.hfrco0().set_bit().dpll0().set_bit());
        let hfrco = &*pac::Hfrco0S::ptr();
        while hfrco.status().read().freqbsy().bit_is_set() {}
        hfrco.cal().write(|w| w.bits(cal));
        while hfrco.status().read().freqbsy().bit_is_set() {}
        cmu.dpllrefclkctrl().write(|w| w.bits(1)); // the reference: HFXO
        let dpll = &*pac::Dpll0S::ptr();
        dpll.en().write(|w| w.bits(0));
        while dpll.en().read().bits() & 0b10 != 0 {} // DISABLING
                                                     // Phase lock (MODE = 1), automatic recovery; f = 39 MHz * (N + 1) / (M + 1)
        dpll.cfg().write(|w| w.bits(1 | 1 << 2));
        dpll.cfg1().write(|w| w.bits(1999 | 3999 << 16));
        ((dpll.if_() as *const _ as usize + 0x2000) as *mut u32).write_volatile(0b111);
        dpll.en().write(|w| w.bits(1));
        // Ready, or a lock failure (IF bits 1, 2); a spin count is the
        // timeout (the CPU runs from the crystal meanwhile)
        let mut locked = false;
        for _ in 0..2_000_000 {
            if dpll.if_().read().bits() & 0b110 != 0 {
                break;
            }
            if dpll.status().read().bits() & 1 != 0 {
                locked = true;
                break;
            }
        }
        if !locked {
            dpll.en().write(|w| w.bits(0));
            return false;
        }
        // PCLK = HCLK / 2 before HCLK goes above PCLK's 50 MHz limit
        cmu.sysclkctrl().modify(|r, w| w.bits(r.bits() | 1 << 10));
        cmu.sysclkctrl()
            .modify(|r, w| w.bits(r.bits() & !0b111 | 2)); // HFRCODPLL
    }
    true
}
