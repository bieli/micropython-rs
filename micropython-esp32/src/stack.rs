//! How much of the main stack scripts use, measured by painting: the unused
//! part of the stack is filled with a pattern, and the lowest word that no
//! longer holds it shows how deep the stack went since. Scripts read it with
//! `stack_peak()` (see the README's stack budget).

use core::ptr::addr_of;

extern "C" {
    // The main stack, from esp-hal's linker script: it grows down from
    // `_stack_start_cpu0` towards its end, where esp-hal keeps a canary word
    // (`__stack_chk_guard`, 60 bytes above the end) under a hardware
    // watchpoint: a write to it stops the board with "Stack corruption
    // detected". Painting stays above it.
    static _stack_start_cpu0: u8;
    static __stack_chk_guard: u32;
}

const PAINT: u32 = 0x5AA5_C33C;
/// Left unpainted below the painting function's own frame, for the register
/// window spills of the code it calls.
const MARGIN: usize = 512;

/// (lowest, highest) address of the part of the main stack that's measured:
/// from just above the canary to the top.
fn bounds() -> (usize, usize) {
    // Only the addresses of the symbols are taken (they're never read)
    (
        addr_of!(__stack_chk_guard) as usize + 4,
        addr_of!(_stack_start_cpu0) as usize,
    )
}

/// Paints the stack below the caller, so that `peak()` measures from now on.
#[inline(never)]
pub fn paint() {
    // A local (a `&0u8` literal would be a constant in flash, not on the stack)
    let marker = 0u8;
    let here = core::hint::black_box(&marker) as *const u8 as usize;
    let (bottom, _) = bounds();
    let mut p = (bottom + 3) & !3;
    let end = (here - MARGIN) & !3;
    // Interrupts off: their handlers push onto this stack, below us
    critical_section::with(|_| {
        while p < end {
            // SAFETY: [bottom, here - MARGIN) is the unused part of the main
            // stack (below every live frame), with interrupts disabled
            unsafe { (p as *mut u32).write_volatile(PAINT) };
            p += 4;
        }
    });
}

/// Most bytes of the main stack in use since the last `paint()` (or boot).
pub fn peak() -> usize {
    let (bottom, top) = bounds();
    let mut p = (bottom + 3) & !3;
    // SAFETY: reads words of the main stack between its bounds
    while p < top && unsafe { (p as *const u32).read_volatile() } == PAINT {
        p += 4;
    }
    top - p
}

/// `stack_peak()` for scripts: the peak since the previous call, then paints
/// again, so each call measures the code between it and the previous one.
pub fn peak_and_repaint() -> usize {
    let used = peak();
    paint();
    used
}
