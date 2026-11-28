//! Native speed tests for `machine.bench()`: how fast this chip runs integer
//! and float arithmetic, moves memory and drives its pins, with no
//! interpreter in between (`cpu_bench(name)`, see `board.rs`).
//!
//! Each test is a chain of operations where every step needs the result of
//! the previous one, so the compiler can neither drop nor fold it (adding a
//! constant eight times would become one multiplication; a Fibonacci step
//! can't). Chains measure an operation's latency, the time code waits for
//! its result. A test runs with 256, 512, 1024, ... repeats until it takes
//! at least 20 ms, so the same code fits an 80 MHz C3 and a 240 MHz S3, and
//! returns operations (bytes, toggles) per second.
//!
//! What this chip has (EFR32MG24, Arm Cortex-M33 with the DSP extension):
//! 32-bit multiply in one cycle and a hardware divider, count leading zeros
//! in one instruction (CLZ), a single precision FPU (add, multiply, divide
//! and square root in hardware); doubles run in software. Copied from
//! micropython-esp32's bench.rs, with this chip's timer and pins.

use core::hint::black_box;

/// Runs `body(n)` (which does `n` repeats and returns how many operations,
/// bytes or toggles that was) with growing `n` until it takes 20 ms, and
/// returns the rate per second.
fn rate(mut body: impl FnMut(u32) -> u64) -> i64 {
    let mut n = 256u32;
    loop {
        let start = crate::clock::now_us();
        let done = body(n);
        let us = (crate::clock::now_us() - start).max(1);
        if us >= 20_000 || n >= 1 << 26 {
            return (done * 1_000_000 / us) as i64;
        }
        // Aim at 25 ms next (at most 16x more at once)
        let scale = (25_000 / us).clamp(2, 16) as u32;
        n = n.saturating_mul(scale);
    }
}

/// Eight copies of a dependent step per loop pass: the loop itself costs
/// little next to them.
macro_rules! eight {
    ($step:expr) => {{
        $step;
        $step;
        $step;
        $step;
        $step;
        $step;
        $step;
        $step;
    }};
}

/// Sums `words`, 8 at a time: the firmware is built for size (no loop
/// unrolling by the compiler), and a loop around every word would measure
/// the loop, not the memory.
#[inline(always)]
fn sum_words(words: &[u32]) -> u32 {
    let mut sum = 0u32;
    let (chunks, rest) = words.as_chunks::<8>();
    for c in chunks {
        sum = sum
            .wrapping_add(c[0])
            .wrapping_add(c[1])
            .wrapping_add(c[2])
            .wrapping_add(c[3])
            .wrapping_add(c[4])
            .wrapping_add(c[5])
            .wrapping_add(c[6])
            .wrapping_add(c[7]);
    }
    for &w in rest {
        sum = sum.wrapping_add(w);
    }
    sum
}

/// The words of the frozen Python modules (in flash): what the flash read
/// test reads, through the flash cache.
fn flash_words() -> &'static [u32] {
    let text = crate::board::frozen_text().as_bytes();
    // SAFETY: u32 has no invalid bit patterns; align_to gives the aligned middle
    let (_, words, _) = unsafe { text.align_to::<u32>() };
    words
}

/// A native test by name: operations (or bytes) per second, or `None` for
/// an unknown name. GPIO tests are in `Board::gpio_bench`.
pub fn run(name: &str) -> Option<i64> {
    let r = match name {
        // --- integers ---
        "i32_add" => rate(|n| {
            let (mut a, mut b) = (black_box(1u32), black_box(1u32));
            for _ in 0..n {
                eight!({
                    let c = a.wrapping_add(b);
                    a = b;
                    b = c;
                });
            }
            black_box(b);
            n as u64 * 8
        }),
        "i32_mul" => rate(|n| {
            let mut x = black_box(1u32);
            for i in 0..n {
                eight!(x = (x ^ i).wrapping_mul(0x0100_0193));
            }
            black_box(x);
            n as u64 * 8
        }),
        "i32_div" => rate(|n| {
            let mut x = black_box(7u32);
            for i in 0..n {
                eight!(x = (0xffff_fff0u32 / (x | 1)).wrapping_add(i));
            }
            black_box(x);
            n as u64 * 8
        }),
        "i64_add" => rate(|n| {
            let (mut a, mut b) = (black_box(1u64), black_box(1u64));
            for _ in 0..n {
                eight!({
                    let c = a.wrapping_add(b);
                    a = b;
                    b = c;
                });
            }
            black_box(b);
            n as u64 * 8
        }),
        "i64_mul" => rate(|n| {
            let mut x = black_box(1u64);
            for i in 0..n {
                eight!(x = (x ^ i as u64).wrapping_mul(0x0000_0100_0000_01b3));
            }
            black_box(x);
            n as u64 * 8
        }),
        "i64_div" => rate(|n| {
            let mut x = black_box(7u64);
            for i in 0..n {
                eight!(x = (0xffff_ffff_ffff_fff0u64 / (x | 1)).wrapping_add(i as u64));
            }
            black_box(x);
            n as u64 * 8
        }),
        "clz" => rate(|n| {
            let mut x = black_box(0x1234_5678u32);
            for i in 0..n {
                eight!(x = (x ^ i).leading_zeros().wrapping_add(x << 1));
            }
            black_box(x);
            n as u64 * 8
        }),
        // --- single precision ---
        "f32_add" => rate(|n| {
            let (mut a, b) = (black_box(1.0f32), black_box(1.0e-7f32));
            for _ in 0..n {
                eight!(a += b);
            }
            black_box(a);
            n as u64 * 8
        }),
        "f32_mul" => rate(|n| {
            let (mut a, b) = (black_box(1.0f32), black_box(1.000_000_1f32));
            for _ in 0..n {
                eight!(a *= b);
            }
            black_box(a);
            n as u64 * 8
        }),
        "f32_div" => rate(|n| {
            let (mut a, b) = (black_box(1.5f32), black_box(2.25f32));
            for _ in 0..n {
                eight!(a = b / a);
            }
            black_box(a);
            n as u64 * 8
        }),
        "f32_sqrt" => rate(|n| {
            let mut a = black_box(2.0f32);
            for _ in 0..n {
                eight!(a = libm::sqrtf(a + 1.0));
            }
            black_box(a);
            n as u64 * 8
        }),
        "f32_sin" => rate(|n| {
            let mut a = black_box(0.5f32);
            for _ in 0..n {
                eight!(a = libm::sinf(a + 0.1));
            }
            black_box(a);
            n as u64 * 8
        }),
        // --- double precision ---
        "f64_add" => rate(|n| {
            let (mut a, b) = (black_box(1.0f64), black_box(1.0e-9f64));
            for _ in 0..n {
                eight!(a += b);
            }
            black_box(a);
            n as u64 * 8
        }),
        "f64_mul" => rate(|n| {
            let (mut a, b) = (black_box(1.0f64), black_box(1.000_000_000_1f64));
            for _ in 0..n {
                eight!(a *= b);
            }
            black_box(a);
            n as u64 * 8
        }),
        "f64_div" => rate(|n| {
            let (mut a, b) = (black_box(1.5f64), black_box(2.25f64));
            for _ in 0..n {
                eight!(a = b / a);
            }
            black_box(a);
            n as u64 * 8
        }),
        "f64_sqrt" => rate(|n| {
            let mut a = black_box(2.0f64);
            for _ in 0..n {
                eight!(a = libm::sqrt(a + 1.0));
            }
            black_box(a);
            n as u64 * 8
        }),
        "f64_sin" => rate(|n| {
            let mut a = black_box(0.5f64);
            for _ in 0..n {
                eight!(a = libm::sin(a + 0.1));
            }
            black_box(a);
            n as u64 * 8
        }),
        // --- memory (a 4 KB buffer on the stack, in internal SRAM) ---
        "ram_write" => {
            let mut buf = [0u32; 1024];
            rate(|n| {
                for i in 0..n {
                    // memset (compiler-builtins, word stores)
                    // SAFETY: writes the buffer's 4096 bytes
                    unsafe { core::ptr::write_bytes(buf.as_mut_ptr(), i as u8, buf.len()) };
                    black_box(&mut buf);
                }
                n as u64 * 4096
            })
        }
        "ram_read" => {
            let mut buf = [0u32; 1024];
            for (i, w) in buf.iter_mut().enumerate() {
                *w = i as u32;
            }
            let buf = black_box(buf);
            rate(|n| {
                let mut sum = 0u32;
                for _ in 0..n {
                    sum = sum.wrapping_add(sum_words(black_box(&buf)));
                }
                black_box(sum);
                n as u64 * 4096
            })
        }
        "ram_copy" => {
            let mut buf = [0u32; 1024];
            rate(|n| {
                for _ in 0..n {
                    let (a, b) = buf.split_at_mut(512);
                    b.copy_from_slice(a);
                    black_box(&mut buf);
                }
                n as u64 * 2048
            })
        }
        "flash_read" => {
            let words = flash_words();
            if words.is_empty() {
                return None;
            }
            rate(|n| {
                let mut sum = 0u32;
                for _ in 0..n {
                    sum = sum.wrapping_add(sum_words(black_box(words)));
                }
                black_box(sum);
                n as u64 * words.len() as u64 * 4
            })
        }
        _ => return None,
    };
    Some(r)
}

/// Pin toggles (or reads) per second: through `gpio::toggle` (what
/// `pin_toggle` uses) or straight to the port's DOUT/DIN registers.
pub fn gpio(id: crate::gpio::PinId, how: &str) -> Option<i64> {
    let (dout, mask) = crate::gpio::dout_reg(id);
    let (din, in_mask) = crate::gpio::din_reg(id);
    let r = match how {
        "gpio_hal" => rate(|n| {
            for _ in 0..n {
                eight!(crate::gpio::toggle(id));
            }
            n as u64 * 8
        }),
        "gpio_reg" => rate(|n| {
            for _ in 0..n {
                // SAFETY: the pin's port DOUT register (an output pin)
                eight!(unsafe { dout.write_volatile(dout.read_volatile() ^ mask) });
            }
            n as u64 * 8
        }),
        "gpio_read" => rate(|n| {
            let mut sum = 0u32;
            for _ in 0..n {
                // SAFETY: reads the pin's port DIN register
                eight!(sum = sum.wrapping_add(unsafe { din.read_volatile() } & in_mask));
            }
            black_box(sum);
            n as u64 * 8
        }),
        _ => return None,
    };
    Some(r)
}
