//! The firmware's frozen `time` module (micropython-esp32/modules/time.py, on
//! the native `utime`, whose calendar is micropython_rs::calendar) and `ntptime`
//! (modules/ntptime.py), against a mock clock: MicroPython's ESP32 epoch
//! (2000-01-01), time tuples as MicroPython gives them.
#![cfg(all(feature = "modules", feature = "classes", feature = "format"))]

use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Interpreter, Module, Output, Returns};
use std::fmt::Write;

const FROZEN: &[(&str, &str)] = &[
    ("time", include_str!("../micropython-esp32/modules/time.py")),
    (
        "ntptime",
        include_str!("../micropython-esp32/modules/ntptime.py"),
    ),
];

/// The firmware's native `utime` (board.rs, with the `machine` feature).
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
        HwFn::with_args("gmtime", 0, 1, "[secs]", "").returning_values(),
        HwFn::with_args("localtime", 0, 1, "[secs]", "").returning_values(),
        HwFn::new("_mktime", "year, month, mday, hour, minute, second", ""),
    ],
    constants: &[],
};

const FUNCTIONS: &[HwFn] = &[
    HwFn::with_args("rtc_set", 1, 2, "seconds, [microseconds]", ""),
    HwFn::with_args("wifi_ntp", 1, 2, "host, [timeout_ms]", ""),
];

/// A clock in microseconds since 2000-01-01, and an NTP server that answers
/// 2026-11-25 23:30:06.25 UTC (or not, for `unreachable`).
struct MockClock {
    log: String,
    clock_us: i64,
    unreachable: bool,
}

impl HardwareBridge for MockClock {
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        match name {
            "rtc_set" => {
                let us = args.get(1).map_or(Ok(0), |a| a.int())?;
                self.clock_us = args[0].int()? * 1_000_000 + us;
                writeln!(self.log, "rtc_set({}, {})", args[0].int()?, us).unwrap();
                Ok(None)
            }
            "wifi_ntp" => {
                let timeout = args.get(1).map_or(Ok(-1), |a| a.int())?;
                writeln!(self.log, "wifi_ntp({:?}, {})", args[0].str()?, timeout).unwrap();
                if self.unreachable {
                    return Err(Error::IoError);
                }
                Ok(Some(848_964_606_250_000))
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }

    fn call_module(
        &mut self,
        _module: &str,
        name: &str,
        args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        match name {
            "time" => Ok(Some(self.clock_us.div_euclid(1_000_000))),
            "time_ns" => Ok(Some(self.clock_us * 1000)),
            "sleep_ms" => {
                self.clock_us += args[0].int()? * 1000;
                Ok(None)
            }
            "ticks_add" => Ok(Some(args[0].int()? + args[1].int()?)),
            "_mktime" => {
                let v = |i: usize| args[i].int();
                Ok(Some(micropython_rs::calendar::mktime(
                    v(0)?,
                    v(1)?,
                    v(2)?,
                    v(3)?,
                    v(4)?,
                    v(5)?,
                )))
            }
            _ => Ok(Some(0)),
        }
    }

    fn call_values(
        &mut self,
        _module: Option<&str>,
        _name: &str,
        args: &[Arg<'_>],
        out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        // utime.gmtime / localtime, as in the firmware (board.rs)
        let secs = match args.first() {
            Some(arg) => arg.int()?,
            None => self.clock_us.div_euclid(1_000_000),
        };
        let t = micropython_rs::calendar::gmtime(secs);
        out.begin_tuple()?;
        for v in [
            t.year, t.month, t.mday, t.hour, t.minute, t.second, t.weekday, t.yearday,
        ] {
            out.int(v)?;
        }
        out.end()
    }

    fn functions(&self) -> &[HwFn] {
        FUNCTIONS
    }

    fn modules(&self) -> &[Module] {
        &[UTIME]
    }
}

struct Text(String);

impl Output for Text {
    fn write_str(&mut self, s: &str) {
        self.0.push_str(s);
    }
}

fn run_with(script: &str, clock_us: i64, unreachable: bool) -> (String, String) {
    let mut out = Text(String::new());
    let mut clock = MockClock {
        log: String::new(),
        clock_us,
        unreachable,
    };
    let mut heap = vec![0u8; 32 * 1024];
    let mut vm = Interpreter::with_hardware(&mut out, &mut clock)
        .with_heap(&mut heap)
        .with_frozen_modules(FROZEN);
    if let Err(err) = vm.eval(script) {
        let mut message = Text(String::new());
        vm.write_exception(err, &mut message);
        panic!("{} at line {:?}", message.0, vm.error_line());
    }
    let _ = vm; // ends the borrows of `out` and `clock`
    (out.0, clock.log)
}

fn run(script: &str) -> String {
    run_with(script, 0, false).0
}

#[test]
fn gmtime_like_micropython_esp32() {
    // Epoch 2000-01-01 (a Saturday: weekday 5), Monday is 0, yearday from 1
    let out = run("import time\n\
         print(time.gmtime(0))\n\
         print(time.gmtime(848964606))\n\
         print(time.gmtime(-1))\n\
         print(time.gmtime(time.mktime((2024, 2, 29, 12, 0, 0, 0, 0))))\n\
         print(time.gmtime(time.mktime((2100, 3, 1, 0, 0, 0, 0, 0)))[:3])\n\
         print(time.localtime(86400 * 366) == time.gmtime(86400 * 366))\n");
    assert_eq!(
        out,
        "(2000, 1, 1, 0, 0, 0, 5, 1)\n\
         (2026, 11, 25, 23, 30, 6, 2, 329)\n\
         (1999, 12, 31, 23, 59, 59, 4, 365)\n\
         (2024, 2, 29, 12, 0, 0, 3, 60)\n\
         (2100, 3, 1)\n\
         True\n"
    );
}

#[test]
fn mktime_round_trips_and_normalizes_months() {
    let out = run("import time\n\
         ok = True\n\
         t = -400000000\n\
         while t < 3000000000:\n    \
             if time.mktime(time.gmtime(t)) != t:\n        \
                 ok = False\n    \
             t += 7654321\n\
         print(ok)\n\
         print(time.mktime((2026, 13, 1, 0, 0, 0, 0, 0)) == time.mktime((2027, 1, 1, 0, 0, 0, 0, 0)))\n\
         print(time.mktime((2026, 0, 1, 0, 0, 0, 0, 0)) == time.mktime((2025, 12, 1, 0, 0, 0, 0, 0)))\n");
    assert_eq!(out, "True\nTrue\nTrue\n");
}

#[test]
fn time_reads_the_native_clock() {
    // 2026-11-25 23:30:06.5 on the board's clock
    let (out, _) = run_with(
        "import time, utime\n\
         print(time.time(), time.time_ns(), time.localtime()[:6])\n\
         time.sleep_ms(1500)\n\
         print(time.time(), utime.time(), time.ticks_add(5, 7))\n",
        848_964_606_500_000,
        false,
    );
    assert_eq!(
        out,
        "848964606 848964606500000000 (2026, 11, 25, 23, 30, 6)\n848964608 848964608 12\n"
    );
}

#[test]
fn ntptime_sets_the_clock() {
    let (out, log) = run_with(
        "import ntptime, time\n\
         print(ntptime.host, ntptime.time())\n\
         ntptime.host = 'pl.pool.ntp.org'\n\
         ntptime.timeout = 2\n\
         ntptime.settime()\n\
         print(time.gmtime())\n",
        0,
        false,
    );
    assert_eq!(
        out,
        "pool.ntp.org 848964606\n(2026, 11, 25, 23, 30, 6, 2, 329)\n"
    );
    assert_eq!(
        log,
        "wifi_ntp(\"pool.ntp.org\", 1000)\nwifi_ntp(\"pl.pool.ntp.org\", 2000)\nrtc_set(848964606, 250000)\n"
    );
}

#[test]
fn ntptime_failure_is_an_oserror() {
    let (out, _) = run_with(
        "import ntptime, time\n\
         try:\n    ntptime.settime()\nexcept OSError:\n    print('OSError', time.time())\n",
        5_000_000,
        true,
    );
    assert_eq!(out, "OSError 5\n");
}
