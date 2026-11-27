//! The REPL on a mock board: what the ESP32 firmware does over its serial
//! port, on the host (the normal REPL, the raw REPL that mpremote and
//! tools/mpy.py use, `#@version`, the `#@` file commands, soft resets).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::mem::MaybeUninit;
use std::sync::Mutex;

use micropython_port::{BootScript, Memory, Port};
use micropython_rs::flatfs::{FlatFs, RamFlash};
use micropython_rs::interpreter::FuncDef;
use micropython_rs::{NoHardware, Output, StmtCache};

/// What a test types, and everything the board writes (console and output).
static INPUT: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
static WRITTEN: Mutex<String> = Mutex::new(String::new());

struct MockPort;

impl Port for MockPort {
    const NAME: &'static str = "MockChip";
    const VERSION: &'static str = "0.1.0";
    const COMMIT: &'static str = "abc1234";

    fn build_options(out: &mut dyn core::fmt::Write) -> core::fmt::Result {
        write!(out, "chip=mock level=full features= float=64 mhz=80")
    }

    fn read_byte() -> Option<u8> {
        let byte = INPUT.lock().unwrap().pop_front();
        // The script is over: end the REPL's endless loop (the test thread
        // panics, the test reads what was written)
        byte.or_else(|| panic!("end of input"))
    }

    fn discard_through(byte: u8) {
        let mut input = INPUT.lock().unwrap();
        while let Some(b) = input.pop_front() {
            if b == byte {
                break;
            }
        }
    }

    fn console_write(s: &str) {
        WRITTEN.lock().unwrap().push_str(s);
    }

    fn stack_floor() -> usize {
        0
    }
}

struct Screen;

impl Output for Screen {
    fn write_str(&mut self, s: &str) {
        WRITTEN.lock().unwrap().push_str(s);
    }
}

fn leak<T>(value: T) -> &'static mut T {
    Box::leak(Box::new(value))
}

/// Types `input` into a board that boots `boot`, then returns everything it wrote.
fn session(boot: &'static str, input: &[u8]) -> String {
    *INPUT.lock().unwrap() = input.iter().copied().collect();
    WRITTEN.lock().unwrap().clear();
    let memory = Memory {
        arena: leak([0u8; 16 * 1024]),
        heap: leak([0u8; 32 * 1024]),
        modules: leak([0u8; 8 * 1024]),
        stmt_cache: Box::leak(
            (0..8)
                .map(|_| MaybeUninit::<StmtCache<'static>>::uninit())
                .collect(),
        ),
        func_slots: Box::leak(
            (0..64)
                .map(|_| MaybeUninit::<Option<FuncDef<'static>>>::uninit())
                .collect(),
        ),
        frozen: &[(
            "greet",
            "def hello():\n    return 'hi from a frozen module'\n",
        )],
    };
    let result = std::thread::spawn(move || {
        let flash = leak([0xFFu8; 64 * 1024]);
        let files = leak(RefCell::new(FlatFs::mount(RamFlash::new(flash)).unwrap()));
        let files = &*files;
        let boot = BootScript {
            source: boot,
            name: "boot.py",
        };
        micropython_port::run::<MockPort, _, _, _>(
            &mut Screen,
            &mut NoHardware,
            &boot,
            Some(files),
            memory,
            None,
        )
    })
    .join();
    assert!(result.is_err(), "the REPL returned");
    WRITTEN.lock().unwrap().replace("\r\n", "\n")
}

/// Tests share INPUT and WRITTEN: one at a time
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

#[test]
fn boot_script_banner_and_normal_repl() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let out = session(
        "print('booted')\n",
        b"x = 6 * 7\rprint(x)\rfor i in range(2):\r    print(i)\r\r",
    );
    assert!(out.starts_with("boot.py (built-in)\nbooted\n"), "{}", out);
    assert!(
        out.contains("micropython-rs 0.1.0 (abc1234) REPL on MockChip\n"),
        "{}",
        out
    );
    assert!(out.contains(">>> print(x)\n42\n"), "{}", out);
    assert!(out.contains("... \n0\n1\n>>> "), "{}", out);
}

#[test]
fn raw_repl_runs_code_and_reports_errors() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let out = session(
        "",
        b"\x01print(6 * 7)\x04import greet\nprint(greet.hello())\x041 / 0\x04\x02",
    );
    assert!(
        out.contains("raw REPL; CTRL-B to exit\n>OK42\n\x04\x04>"),
        "{}",
        out
    );
    assert!(
        out.contains("OKhi from a frozen module\n\x04\x04>"),
        "{}",
        out
    );
    assert!(
        out.contains("OK\x04Traceback (most recent call last):"),
        "{}",
        out
    );
    assert!(out.contains("ZeroDivisionError"), "{}", out);
}

#[test]
fn version_and_file_commands() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let out = session(
        "",
        b"\x01#@version\x04#@open notes.txt\x04#@write 6869\x04#@close\x04#@ls\x04#@get notes.txt\x04\
          #@save\nprint('from main.py')\x04\x04",
    );
    assert!(
        out.contains("OKversion=0.1.0 commit=abc1234 chip=mock level=full features= float=64 mhz=80\n\x04\x04>"),
        "{}",
        out
    );
    assert!(out.contains("OK2\x04\x04>"), "{}", out); // #@close: 2 bytes
    assert!(out.contains("notes.txt 2\n# 1 of"), "{}", out);
    assert!(out.contains("OK6869\x04\x04>"), "{}", out); // #@get: hex
    assert!(out.contains("saved main.py ("), "{}", out);
    // The empty Ctrl-D in the raw REPL is a soft reset (it skips main.py)
    assert!(
        out.contains("OK\nMPY: soft reboot\nraw REPL; CTRL-B to exit\n>"),
        "{}",
        out
    );
}

#[test]
fn soft_reset_runs_saved_main_py() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // Save main.py in the raw REPL, back to the normal REPL, Ctrl-D: it runs
    let out = session(
        "print('built-in')\n",
        b"\x01#@save\nprint('from main.py')\x04\x02\x04",
    );
    assert!(
        out.contains("MPY: soft reboot\nmain.py from flash ("),
        "{}",
        out
    );
    assert!(
        out.contains("built-in boot.py not used\nfrom main.py\n"),
        "{}",
        out
    );
}
