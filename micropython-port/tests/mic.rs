//! The `mic` module on a simulated microphone: a sine wave on a DC bias, as
//! a 12-bit ADC sees it, run from Python through the interpreter.

use std::cell::RefCell;
use std::rc::Rc;

use micropython_port::mic::{self, Mic, MicHw};
use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Interpreter, Module, Output, Returns};

const SRC_HZ: f64 = 39e6;
const BIAS: f64 = 700.0;
const MV: u32 = 3300;

#[derive(Default)]
struct Log {
    power: Vec<(i64, bool)>,
    inputs: Vec<i64>,
    sampled: Vec<(i64, u32, usize)>,
}

/// A sine of `amp` counts at `tone_hz` around `BIAS`; time runs on across calls.
struct FakeAdc {
    amp: f64,
    tone_hz: f64,
    t: f64,
    log: Rc<RefCell<Log>>,
}

impl MicHw for FakeAdc {
    fn power(&mut self, pin: i64, on: bool) -> Result<(), Error> {
        self.log.borrow_mut().power.push((pin, on));
        Ok(())
    }
    fn analog_input(&mut self, pin: i64) -> Result<(), Error> {
        self.log.borrow_mut().inputs.push(pin);
        Ok(())
    }
    fn actual_rate(&mut self, rate_hz: u32) -> Result<f64, Error> {
        Ok(SRC_HZ / (SRC_HZ / rate_hz as f64).round())
    }
    fn sample(
        &mut self,
        pin: i64,
        rate_hz: u32,
        n: usize,
        each: &mut dyn FnMut(u16),
    ) -> Result<(), Error> {
        self.log.borrow_mut().sampled.push((pin, rate_hz, n));
        let dt = 1.0 / self.actual_rate(rate_hz)?;
        for _ in 0..n {
            let v = BIAS + self.amp * (2.0 * std::f64::consts::PI * self.tone_hz * self.t).sin();
            each(v.round().clamp(0.0, 4095.0) as u16);
            self.t += dt;
        }
        Ok(())
    }
    fn full_scale(&mut self) -> (u16, u32) {
        (4095, MV)
    }
    fn delay_ms(&mut self, _ms: u32) {}
}

const MIC: Module = Module {
    name: "mic",
    doc: "",
    functions: mic::FUNCTIONS,
    constants: &[("PIN", 41), ("POWER", 40)],
};

struct Bridge {
    mic: Mic,
    adc: FakeAdc,
}

impl HardwareBridge for Bridge {
    fn call_args(&mut self, _name: &str, _args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        Err(Error::UnknownIdentifier)
    }
    fn call_values(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[Arg<'_>],
        out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        match module {
            Some("mic") => self.mic.call(&mut self.adc, name, args, out),
            _ => Err(Error::UnknownIdentifier),
        }
    }
    fn functions(&self) -> &[HwFn] {
        &[]
    }
    fn modules(&self) -> &[Module] {
        &[MIC]
    }
}

struct Text(String);

impl Output for Text {
    fn write_str(&mut self, s: &str) {
        self.0.push_str(s);
    }
}

fn run_with(script: &str, amp: f64, log: &Rc<RefCell<Log>>) -> Result<String, String> {
    let mut out = Text(String::new());
    let mut bridge = Bridge {
        mic: Mic::new(41, Some(40), 16_000, 50),
        adc: FakeAdc {
            amp,
            tone_hz: 1000.0,
            t: 0.0,
            log: log.clone(),
        },
    };
    let mut heap = vec![0u8; 96 * 1024];
    let mut table = vec![micropython_rs::heap::HeapEntry::FREE; 4096];
    let mut vm = Interpreter::with_hardware(&mut out, &mut bridge)
        .with_heap(&mut heap)
        .with_heap_table(&mut table);
    let result = vm.eval(script);
    let mut message = Text(String::new());
    if let Err(err) = result {
        vm.write_exception(err, &mut message);
    }
    let _ = vm;
    if message.0.is_empty() {
        Ok(out.0)
    } else {
        Err(message.0)
    }
}

fn run(script: &str, amp: f64) -> (String, Rc<RefCell<Log>>) {
    let log = Rc::new(RefCell::new(Log::default()));
    let out = run_with(script, amp, &log).unwrap_or_else(|e| panic!("{}", e));
    (out, log)
}

/// Python helpers: signed 16-bit samples from bytes, their mean and peak.
const PCM: &str = "
def samples(b):
    out = []
    for i in range(0, len(b), 2):
        v = b[i] | (b[i + 1] << 8)
        out.append(v - 65536 if v >= 32768 else v)
    return out
";

#[test]
fn record_removes_the_bias_and_scales_to_16_bits() {
    let (out, log) = run(
        &format!(
            "{PCM}import mic\nb = mic.record(1600)\ns = samples(b)\n\
             print(len(b), abs(sum(s) / len(s)) < 20, max(s), min(s))"
        ),
        100.0,
    );
    // 100 counts of a 12-bit ADC: 1600 of 32767 (times 16), the mean ~0
    assert_eq!(out, "3200 True 1600 -1600\n");
    let log = log.borrow();
    // Started on first use: the analog pin, then power on
    assert_eq!(log.inputs, [41]);
    assert_eq!(log.power, [(40, true)]);
    assert_eq!(log.sampled, [(41, 16_000, 1600)]);
}

#[test]
fn gain_multiplies_and_clips() {
    let (out, _) = run(
        &format!("{PCM}import mic\ns = samples(mic.record(800, 8))\nprint(max(s), min(s))"),
        100.0,
    );
    assert_eq!(out, "12800 -12800\n");
    let (out, _) = run(
        &format!(
            "{PCM}import mic\ns = samples(mic.record(800, 256))\n\
             print(max(s), min(s), s.count(32767) > 100)"
        ),
        100.0,
    );
    assert_eq!(out, "32767 -32767 True\n");
}

#[test]
fn wav_is_a_playable_file() {
    let (out, _) = run(
        "import mic\nmic.init(41, 40, 8000)\nw = mic.wav(100)\n\
         print(len(w), w[:4], w[8:16], w[36:40])\n\
         print(w[24] | w[25] << 8, w[40] | w[41] << 8, w[22], w[34])",
        50.0,
    );
    // 44 + 200 bytes; 8000 Hz, 200 data bytes, mono, 16 bits
    assert_eq!(out, "244 b'RIFF' b'WAVEfmt ' b'data'\n8000 200 1 16\n");
}

#[test]
fn wav_header_fields() {
    let mut h = [0u8; mic::WAV_HEADER];
    mic::wav_header(&mut h, 16_003, 32_000);
    assert_eq!(&h[0..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 36 + 32_000);
    assert_eq!(&h[8..16], b"WAVEfmt ");
    assert_eq!(u32::from_le_bytes(h[16..20].try_into().unwrap()), 16);
    assert_eq!(u16::from_le_bytes([h[20], h[21]]), 1);
    assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 16_003);
    assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 32_006);
    assert_eq!(u16::from_le_bytes([h[32], h[33]]), 2);
    assert_eq!(&h[36..40], b"data");
    assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 32_000);
}

#[test]
fn level_in_millivolts() {
    let (out, _) = run(
        "import mic\nrms, peak, dc = mic.level(100)\n\
         print(round(rms), round(peak), round(dc))",
        400.0,
    );
    // 400 counts of 4095 at 3300 mV: peak 322 mV, rms 322 / sqrt(2) = 228
    // mV, the bias 700 counts = 564 mV
    assert_eq!(out, "228 322 564\n");
    let (out, _) = run("import mic\nprint(mic.level(10)[0] < 0.5)", 0.0);
    assert_eq!(out, "True\n");
}

#[test]
fn raw_rate_init_and_off() {
    let (out, log) = run(
        "import mic\nprint(mic.init(5, -1, 8000), mic.rate())\n\
         r = mic.raw(8)\nprint(len(r), r[0], min(r) >= 600, max(r) <= 800)\n\
         mic.off()\nmic.init()\nmic.off()\nprint(mic.PIN, mic.POWER)",
        100.0,
    );
    // 39 MHz / 8000 is a whole number of cycles: exactly 8000 Hz
    assert_eq!(out, "8000.0 8000.0\n8 700 True True\n41 40\n");
    let log = log.borrow();
    // No power pin for the first one; init() keeps the last settings
    assert_eq!(log.inputs, [5, 5]);
    assert!(log.power.is_empty(), "{:?}", log.power);
}

#[test]
fn off_powers_down_and_the_next_call_restarts() {
    let (_, log) = run("import mic\nmic.level(5)\nmic.off()\nmic.level(5)", 10.0);
    assert_eq!(log.borrow().power, [(40, true), (40, false), (40, true)]);
}

#[test]
fn bad_arguments() {
    for script in [
        "mic.init(41, 40, 500)",
        "mic.init(41, 40, 96000)",
        "mic.record(-1)",
        "mic.record(10, 0)",
        "mic.record(10, 1000)",
        "mic.raw(2000)",
        "mic.level(0)",
    ] {
        let log = Rc::new(RefCell::new(Log::default()));
        let err = run_with(&format!("import mic\n{script}"), 10.0, &log).unwrap_err();
        assert!(err.contains("ValueError"), "{script}: {err}");
    }
    // Too long for the memory: MemoryError, not a crash
    let log = Rc::new(RefCell::new(Log::default()));
    let err = run_with("import mic\nmic.record(100000)", 10.0, &log).unwrap_err();
    assert!(err.contains("MemoryError"), "{err}");
    assert!(log.borrow().sampled.is_empty());
}

#[test]
fn pins_for_tells_the_board_what_a_call_takes() {
    let m = Mic::new(41, Some(40), 16_000, 50);
    assert_eq!(m.pins_for("record", &[]).unwrap(), Some((41, Some(40))));
    assert_eq!(m.pins_for("off", &[]).unwrap(), None);
    assert_eq!(
        m.pins_for("init", &[Arg::Int(3), Arg::Int(-1)]).unwrap(),
        Some((3, None))
    );
}
