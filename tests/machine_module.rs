//! The firmware's frozen `machine` module (micropython-esp32/modules/machine.py)
//! against a mock board that logs every hardware call: MicroPython-style code
//! must turn into the right firmware functions. (It needs the `full` level's
//! frozen modules and classes, like the firmware's `machine` feature.)
#![cfg(all(feature = "modules", feature = "classes", feature = "format"))]

use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Interpreter, Module, Output};
use std::fmt::Write;

const MACHINE: &str = include_str!("../micropython-esp32/modules/machine.py");
const FROZEN: &[(&str, &str)] = &[
    ("machine", MACHINE),
    ("time", include_str!("../micropython-esp32/modules/time.py")),
];

/// The firmware's native `utime`, for `machine.RTC` (through `time`).
const UTIME: Module = Module {
    name: "utime",
    doc: "",
    functions: &[
        HwFn::new("sleep", "s", ""),
        HwFn::new("sleep_ms", "ms", ""),
        HwFn::new("sleep_us", "us", ""),
        HwFn::new("ticks_ms", "", ""),
        HwFn::new("ticks_us", "", ""),
        HwFn::new("ticks_diff", "new, old", ""),
        HwFn::new("ticks_add", "ticks, delta", ""),
        HwFn::new("time", "", ""),
        HwFn::new("time_ns", "", ""),
    ],
    constants: &[],
};

/// Names and argument counts as in the firmware's table (board.rs).
const FUNCTIONS: &[HwFn] = &[
    HwFn::new("sleep_ms", "ms", ""),
    HwFn::new("pin_mode", "pin, mode", ""),
    HwFn::new("pin_write", "pin, value", ""),
    HwFn::new("pin_read", "pin", ""),
    HwFn::new("pin_toggle", "pin", ""),
    HwFn::new("adc_read", "pin", ""),
    HwFn::with_args("adc_mv", 1, 2, "pin, [samples]", ""),
    HwFn::new("pwm_u16", "pin, freq, duty", ""),
    HwFn::new("pwm_stop", "pin", ""),
    HwFn::with_args("i2c_init", 2, 3, "sda, scl, [freq]", ""),
    HwFn::new("i2c_scan", "", "").returning_bytes(),
    HwFn::new("i2c_write", "addr, data", ""),
    HwFn::new("i2c_read", "addr, n", "").returning_bytes(),
    HwFn::new("i2c_write_read", "addr, data, n", "").returning_bytes(),
    HwFn::with_args("uart_init", 2, 3, "tx, rx, [baud]", ""),
    HwFn::new("uart_write", "data", ""),
    HwFn::with_args("uart_read", 0, 2, "[n], [timeout_ms]", "").returning_bytes(),
    HwFn::new("uart_any", "", ""),
    HwFn::with_args("spi_init", 3, 5, "sck, mosi, miso, [khz], [mode]", ""),
    HwFn::new("spi_write", "data", ""),
    HwFn::with_args("spi_read", 1, 2, "n, [fill]", "").returning_bytes(),
    HwFn::new("wdt_enable", "timeout_ms", ""),
    HwFn::new("wdt_feed", "", ""),
    HwFn::new("reset", "", ""),
    HwFn::new("reset_cause", "", ""),
    HwFn::new("unique_id", "", "").returning_bytes(),
    HwFn::new("cpu_freq", "", ""),
    HwFn::new("pin_irq", "pin, trigger", ""),
    HwFn::new("timer_init", "id, period_us, periodic", ""),
    HwFn::new("timer_deinit", "id", ""),
    HwFn::new("fire", "event", ""),
    HwFn::with_args("rtc_set", 1, 2, "seconds, [microseconds]", ""),
    HwFn::new("cpu_info", "", "").returning_bytes(),
    HwFn::with_args("cpu_bench", 1, 2, "name, [pin]", ""),
];

const CONSTANTS: &[(&str, i64)] = &[
    ("OUT", 1),
    ("IN", 2),
    ("IN_PULLUP", 3),
    ("IN_PULLDOWN", 4),
    ("OUT_OD", 5),
    ("OUT_OD_PULLUP", 6),
    ("PWRON_RESET", 1),
    ("HARD_RESET", 2),
    ("WDT_RESET", 3),
    ("DEEPSLEEP_RESET", 4),
    ("SOFT_RESET", 5),
    ("I2C_SDA", 21),
    ("I2C_SCL", 22),
];

#[derive(Default)]
struct MockBoard {
    log: String,
    pins: Vec<i64>,
    /// Bytes the "UART" receives, handed out by uart_read.
    rx: Vec<u8>,
    /// Events for the interpreter (`fire(n)` queues one)
    events: Vec<i64>,
    /// The clock: microseconds since 2000-01-01 (`rtc_set`, `utime.time_ns`)
    clock_us: i64,
}

fn show(args: &[Arg<'_>]) -> String {
    let parts: Vec<String> = args
        .iter()
        .map(|a| match a {
            Arg::Int(n) => n.to_string(),
            Arg::Float(f) => f.to_string(),
            Arg::Str(s) => format!("{:?}", s),
            Arg::Bytes(b) => format!("b{:?}", b),
        })
        .collect();
    parts.join(",")
}

impl HardwareBridge for MockBoard {
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        writeln!(self.log, "{}({})", name, show(args)).unwrap();
        let int = |i: usize| args[i].int();
        match name {
            "pin_write" => {
                self.pins[int(0)? as usize] = int(1)?;
                Ok(None)
            }
            "pin_read" => Ok(Some(self.pins[int(0)? as usize])),
            "pin_toggle" => {
                self.pins[int(0)? as usize] ^= 1;
                Ok(None)
            }
            "adc_read" if int(0)? == 34 => Ok(Some(2048)),
            "adc_read" => Err(Error::ValueError),
            "adc_mv" => Ok(Some(1650)),
            "i2c_write" | "uart_write" => Ok(Some(args[args.len() - 1].bytes()?.len() as i64)),
            "uart_any" => Ok(Some(self.rx.len() as i64)),
            "cpu_freq" => Ok(Some(80_000_000)),
            "fire" => {
                self.events.push(int(0)?);
                Ok(None)
            }
            "reset_cause" => Ok(Some(1)),
            // Native rates: 40 M/s for everything, but no flash text (ValueError)
            "cpu_bench" if args[0].str()? == "flash_read" => Err(Error::ValueError),
            "cpu_bench" => Ok(Some(40_000_000)),
            "rtc_set" => {
                self.clock_us = int(0)? * 1_000_000 + args.get(1).map_or(Ok(0), |a| a.int())?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn call_bytes(&mut self, name: &str, args: &[Arg<'_>], out: &mut [u8]) -> Result<usize, Error> {
        writeln!(self.log, "{}({})", name, show(args)).unwrap();
        let data: Vec<u8> = match name {
            "i2c_scan" => vec![0x3c, 0x68],
            "i2c_read" | "i2c_write_read" | "spi_read" => {
                let n = args[args.len() - 1 - (name == "spi_read") as usize].int()? as usize;
                (0..n as u8).map(|i| 0xa0 + i).collect()
            }
            "uart_read" => {
                let n = (args.first().map_or(Ok(64), |a| a.int())? as usize).min(self.rx.len());
                self.rx.drain(..n.min(5)).collect() // arrives 5 bytes at a time
            }
            "unique_id" => vec![0x24, 0x0a, 0xc4, 1, 2, 3],
            "cpu_info" => b"arch=xtensa\nmodel=ESP32-S3\ncore=Xtensa LX7\nrev=v0.2\ncores=2\nmhz=80\n\
                max_mhz=240\nflags=fpu wifi ble usb-jtag\nflash=8388608\nsram=524288\nstack=144308\n\
                mac=d8:3b:da:75:f0:a4\nboard=XIAO\nfirmware=0.1.0\nbuild=full wifi\nuptime_ms=1234\n\
                reset=5\nwifi_heap=73728\nwifi_heap_free=40000\n"
                .to_vec(),
            _ => return Err(Error::UnknownIdentifier),
        };
        out[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    fn functions(&self) -> &[HwFn] {
        FUNCTIONS
    }

    fn modules(&self) -> &[Module] {
        &[UTIME]
    }

    fn call_module(
        &mut self,
        _module: &str,
        name: &str,
        _args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        match name {
            "time" => Ok(Some(self.clock_us.div_euclid(1_000_000))),
            "time_ns" => Ok(Some(self.clock_us * 1000)),
            _ => Ok(Some(0)),
        }
    }

    fn poll_event(&mut self) -> Option<i64> {
        (!self.events.is_empty()).then(|| self.events.remove(0))
    }

    fn constants(&self) -> &[(&'static str, i64)] {
        CONSTANTS
    }
}

struct Text(String);

impl Output for Text {
    fn write_str(&mut self, s: &str) {
        self.0.push_str(s);
    }
}

/// Runs `script` with the machine module; returns (printed text, hardware log).
fn run(script: &str, rx: &[u8]) -> (String, String) {
    let mut out = Text(String::new());
    let mut board = MockBoard {
        rx: rx.to_vec(),
        pins: vec![0; 40],
        ..Default::default()
    };
    let mut heap = vec![0u8; 32 * 1024];
    let mut vm = Interpreter::with_hardware(&mut out, &mut board)
        .with_heap(&mut heap)
        .with_frozen_modules(FROZEN);
    if let Err(err) = vm.eval(script) {
        let mut message = Text(String::new());
        vm.write_exception(err, &mut message);
        panic!("{} at line {:?}", message.0, vm.error_line());
    }
    let _ = vm; // ends the borrows of `out` and `board`
    (out.0, board.log)
}

#[test]
fn pin() {
    let (out, log) = run(
        "from machine import Pin\n\
         led = Pin(2, Pin.OUT)\nled.on()\nprint(led.value(), led)\nled.off()\nled(1)\nprint(led())\n\
         led.toggle()\nprint(led.value())\n\
         b = Pin(0, Pin.IN, Pin.PULL_UP)\nPin(4, Pin.OPEN_DRAIN, Pin.PULL_UP, value=1)\n\
         Pin(5, mode=Pin.OPEN_DRAIN)\nPin(12, pull=Pin.PULL_DOWN)\nled.value(not led.value())\n\
         try:\n    Pin(2, 99)\nexcept ValueError as e:\n    print('ValueError:', e)\n",
        b"",
    );
    assert_eq!(out, "1 Pin(2)\n1\n0\nValueError: invalid pin mode\n");
    assert_eq!(
        log,
        "pin_mode(2,1)\npin_write(2,1)\npin_read(2)\npin_write(2,0)\npin_write(2,1)\npin_read(2)\n\
         pin_toggle(2)\npin_read(2)\npin_mode(0,3)\npin_mode(4,6)\npin_write(4,1)\npin_mode(5,5)\n\
         pin_mode(12,4)\npin_read(2)\npin_write(2,1)\n"
    );
}

#[test]
fn adc_and_pwm() {
    let (out, log) = run(
        "from machine import ADC, PWM, Pin\n\
         adc = ADC(Pin(34))\nadc.atten(ADC.ATTN_11DB)\nprint(adc.read(), adc.read_u16(), adc.read_uv())\n\
         try:\n    ADC(2)\nexcept ValueError:\n    print('no ADC on 2')\n\
         try:\n    adc.atten(ADC.ATTN_6DB)\nexcept ValueError as e:\n    print(e)\n\
         servo = PWM(Pin(13), freq=50, duty_u16=4915)\nprint(servo.freq(), servo.duty_u16(), servo.duty(), servo.duty_ns())\n\
         servo.duty(512)\nservo.freq(1000)\nservo.duty_ns(250000)\nprint(servo.duty_u16())\n\
         PWM(Pin(2))\nservo.deinit()\n",
        b"",
    );
    assert_eq!(
        out,
        "2048 32776 1650000\nno ADC on 2\nonly ADC.ATTN_11DB (0-3.3 V)\n50 4915 77 1499961\n16383\n"
    );
    assert_eq!(
        log,
        "adc_read(34)\nadc_read(34)\nadc_read(34)\nadc_mv(34)\nadc_read(2)\n\
         pwm_u16(13,50,4915)\npwm_u16(13,50,32799)\npwm_u16(13,1000,32799)\npwm_u16(13,1000,16383)\n\
         pwm_u16(2,5000,32768)\npwm_stop(13)\n"
    );
}

#[test]
fn i2c() {
    let (out, log) = run(
        "from machine import I2C, SoftI2C, Pin\n\
         i2c = I2C(0, scl=Pin(22), sda=Pin(21), freq=400000)\nprint(i2c.scan())\n\
         print(i2c.readfrom_mem(0x68, 0x75, 1), i2c.readfrom(0x3c, 2))\n\
         i2c.writeto_mem(0x68, 0x6b, b'\\x00')\nprint(i2c.writeto(0x3c, b'\\x00\\xaf'))\n\
         i2c.readfrom_mem(0x50, 0x0102, 2, addrsize=16)\n\
         soft = SoftI2C(Pin(25), Pin(26), freq=100000)\nI2C(1)\n",
        b"",
    );
    assert_eq!(out, "[60, 104]\nb'\\xa0' b'\\xa0\\xa1'\n2\n");
    assert_eq!(
        log,
        "i2c_init(21,22,400000)\ni2c_scan()\ni2c_write_read(104,b[117],1)\ni2c_read(60,2)\n\
         i2c_write(104,b[107, 0])\ni2c_write(60,b[0, 175])\ni2c_write_read(80,b[1, 2],2)\n\
         i2c_init(26,25,100000)\ni2c_init(21,22,400000)\n"
    );
}

#[test]
fn uart_spi_wdt_and_module_functions() {
    let (out, log) = run(
        "import machine\nfrom machine import UART, SPI, Pin\n\
         gps = UART(2, baudrate=9600)\nprint(gps.any(), gps.readline(), gps.readline())\n\
         print(gps.read(3), gps.read(), gps.read(), gps.write('AT\\r\\n'))\n\
         u = UART(1, 115200, tx=Pin(25), rx=Pin(26), timeout=100)\n\
         try:\n    UART(1)\nexcept ValueError as e:\n    print(e)\n\
         spi = SPI(1, baudrate=2000000, polarity=1, phase=1, sck=Pin(26), mosi=Pin(27), miso=Pin(12))\n\
         spi.write(b'\\x9f')\nprint(spi.read(3, 0xff))\nSPI(2)\n\
         machine.WDT(timeout=2000).feed()\n\
         print(machine.freq(), machine.unique_id(), machine.reset_cause() == machine.PWRON_RESET)\n\
         machine.freq(80000000)\ntry:\n    machine.freq(240000000)\nexcept ValueError as e:\n    print(e)\n",
        b"$GPGGA,1\r\n$GP2\nrest",
    );
    assert_eq!(
        out,
        "19 b'$GPGGA,1\\r\\n' b'$GP2\\n'\nb'res' b't' None 4\nUART(1) needs tx and rx pins\n\
         b'\\xa0\\xa1\\xa2'\n80000000 b'$\\n\\xc4\\x01\\x02\\x03' True\n\
         the CPU clock is fixed at 80000000 Hz (build with CPU_MHZ=240)\n"
    );
    assert!(
        log.starts_with("uart_init(17,16,9600)\nuart_any()\nuart_read(512,0)\n"),
        "{}",
        log
    );
    assert!(log.contains("uart_init(25,26,115200)\n"), "{}", log);
    assert!(
        log.contains("spi_init(26,27,12,2000,3)\nspi_write(b[159])\nspi_read(3,255)\nspi_init(18,23,19,1000,0)\n"),
        "{}",
        log
    );
    assert!(log.contains("wdt_enable(2000)\nwdt_feed()\n"), "{}", log);
}

#[test]
fn leaves_function_slots_for_scripts() {
    // The module's classes take function slots; scripts must still have room
    let mut script = String::from("import machine\n");
    for i in 0..16 {
        script.push_str(&format!("def f{}():\n    return {}\n", i, i));
    }
    script.push_str("print(f15())\n");
    let (out, _) = run(&script, b"");
    assert_eq!(out, "15\n");
}

#[test]
fn pin_irq_and_timer_callbacks() {
    let (out, log) = run(
        "from machine import Pin, Timer\n\
         hits = []\n\
         p = Pin(25, Pin.OUT)\n\
         p.irq(handler=lambda pin: hits.append(('pin', pin)), trigger=Pin.IRQ_RISING)\n\
         t = Timer(1, period=250, callback=lambda tm: hits.append(('timer', tm is t)))\n\
         fire(25)\nfire(101)\nfire(7)\nx = 1\n\
         print(hits)\n\
         t.deinit()\np.irq(handler=None)\n\
         fire(101)\nfire(25)\ny = 2\nprint(len(hits))\n\
         Timer(-1, mode=Timer.ONE_SHOT, freq=10, callback=lambda tm: hits.append('virtual'))\n\
         fire(104)\nz = 3\nprint(hits[-1])\n\
         p.irq(lambda pin: None)\n\
         try:\n    p.irq(lambda pin: None, trigger=Pin.IRQ_LOW_LEVEL)\nexcept ValueError:\n    print('level: ValueError')\n\
         try:\n    Timer(0, mode=Timer.PERIODIC)\n    Timer(0).init()\nexcept ValueError:\n    print('no period: ValueError')\n",
        b"",
    );
    assert_eq!(
        out,
        "[('pin', Pin(25)), ('timer', True)]\n2\nvirtual\nlevel: ValueError\nno period: ValueError\n"
    );
    assert!(
        log.contains("pin_mode(25,1)\npin_irq(25,1)\ntimer_init(1,250000,1)\n"),
        "{}",
        log
    );
    assert!(log.contains("timer_deinit(1)\npin_irq(25,0)\n"), "{}", log);
    assert!(log.contains("timer_init(-1,100000,0)\n"), "{}", log);
    // The default trigger: both edges
    assert!(log.contains("pin_irq(25,3)\n"), "{}", log);
}

#[test]
fn rtc_datetime() {
    // (year, month, day, weekday, hours, minutes, seconds, subseconds in us),
    // as on MicroPython's ESP32 port; the weekday is ignored when setting
    let (out, log) = run(
        "import machine, time\n\
         rtc = machine.RTC()\n\
         print(rtc.datetime())\n\
         rtc.datetime((2026, 11, 26, 0, 7, 32, 15, 250000))\n\
         print(rtc.datetime(), time.gmtime())\n\
         machine.RTC().init((2027, 3, 28, 6, 1, 0, 0))\n\
         print(time.localtime()[:7])\n",
        b"",
    );
    assert_eq!(
        out,
        "(2000, 1, 1, 5, 0, 0, 0, 0)\n\
         (2026, 11, 26, 3, 7, 32, 15, 250000) (2026, 11, 26, 7, 32, 15, 3, 330)\n\
         (2027, 3, 28, 1, 0, 0, 6)\n"
    );
    assert!(
        log.ends_with("rtc_set(848993535,250000)\nrtc_set(859510800,0)\n"),
        "{}",
        log
    );
}

#[test]
fn lscpu_like_linux() {
    let (out, _) = run(
        "import machine\n\
         info = machine.lscpu()\n\
         print(info['Architecture'], info['CPU(s)'], info['Model name'], info['CPU MHz'], info['Flash size'])\n\
         print(info['Reset cause'], info['Byte Order'], 'Python heap size' in info, info['Wi-Fi heap free'])\n\
         print(list(info)[:4])\n\
         machine.lscpu(True)\n",
        b"",
    );
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("xtensa 2 ESP32-S3 80 8388608"));
    assert_eq!(lines.next(), Some("soft reset Little Endian True 40000"));
    assert_eq!(
        lines.next(),
        Some("['Architecture', 'Byte Order', 'CPU(s)', 'Vendor ID']")
    );
    assert_eq!(lines.next(), Some("Architecture:      xtensa"));
    assert!(out.contains("Flash size:        8 MiB\n"), "{}", out);
    assert!(out.contains("Stack size:        140 KiB\n"), "{}", out);
    assert!(out.contains("Reset cause:       soft reset\n"), "{}", out);
}

#[test]
fn bench_suite() {
    let (out, log) = run(
        "import machine, gc\n\
         r = machine.bench(gpio=None)\n\
         print(r['native i32 add (Mop/s)'], r['native f64 sin (Mop/s)'], r['native RAM copy (MiB/s)'])\n\
         print('native flash read, cached (MiB/s)' in r, 'native GPIO toggle, esp-hal (M/s)' in r, len(r))\n\
         r = machine.bench(gpio=4)\n\
         print(r['native GPIO toggle, register (M/s)'], 'Python pin_write (k/s)' in r, len(r))\n\
         machine.bench(True, gpio=None)\n\
         r = None\ngc.collect()\nbefore = gc.objects_free()\nr = machine.bench(gpio=4)\nr = None\ngc.collect()\n\
         print('objects left behind', before - gc.objects_free())\n",
        b"",
    );
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("40.0 40.0 38.1"));
    assert_eq!(lines.next(), Some("False False 27"));
    assert_eq!(lines.next(), Some("40.0 True 32"));
    assert!(
        out.contains("native i32 add (Mop/s)                       40.0\n"),
        "{}",
        out
    );
    assert!(out.contains("Python dict set (k/s)"), "{}", out);
    assert!(out.ends_with("objects left behind 0\n"), "{}", out);
    // GPIO tests left the pin low
    assert!(log.ends_with("pin_write(4,0)\n"), "{}", log);
}
