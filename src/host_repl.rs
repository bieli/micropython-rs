//! `mpy-cli --repl`: a REPL on stdin/stdout that speaks the same protocol as
//! the ESP32 firmware, the friendly REPL (`>>> `) and MicroPython's raw REPL
//! (Ctrl-A ... Ctrl-D), so tools such as Thonny or mpremote can be tested
//! against the interpreter on a PC (connect them through a pseudo-terminal,
//! see `tools/pty_repl.py`).
//!
//! The session keeps its variables between commands, like a board; Ctrl-D in
//! an empty raw REPL (or at `>>> `) is a soft reboot that starts a new
//! interpreter. Files live in RAM for the whole process.

use micropython_rs::flatfs::{FlatFs, RamFlash, SLOT_SIZE};
use micropython_rs::{Error, FileSystem, Interpreter, Output};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};

const CTRL_A: u8 = 0x01;
const CTRL_B: u8 = 0x02;
const CTRL_C: u8 = 0x03;
const CTRL_D: u8 = 0x04;
const CTRL_E: u8 = 0x05;

static ABORT: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

struct Stdout;

impl Output for Stdout {
    fn write_str(&mut self, s: &str) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }
}

fn write(s: &str) {
    Stdout.write_str(s);
}

/// Bytes from stdin; Ctrl-C while a program runs sets the abort flag.
fn reader() -> Receiver<u8> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut byte = [0u8; 1];
        while stdin.read(&mut byte).is_ok_and(|n| n == 1) {
            if byte[0] == CTRL_C && RUNNING.load(Ordering::Relaxed) {
                ABORT.store(true, Ordering::Relaxed);
                continue;
            }
            if tx.send(byte[0]).is_err() {
                break;
            }
        }
        // stdin closed: the channel closes, and the session ends once
        // everything received so far has run
    });
    rx
}

/// Sources must live as long as the interpreter: a REPL session keeps them.
fn keep(source: Vec<u8>) -> Result<&'static str, Error> {
    let text = String::from_utf8(source).map_err(|_| Error::UnexpectedChar('?'))?;
    Ok(Box::leak(text.into_boxed_str()))
}

type Vm = Interpreter<'static, Stdout>;

fn new_vm(files: &'static RefCell<dyn FileSystem>) -> Vm {
    let out: &'static mut Stdout = Box::leak(Box::new(Stdout));
    let heap: &'static mut [u8] = Box::leak(vec![0u8; 256 * 1024].into_boxed_slice());
    let mut vm = Interpreter::new(out).with_heap(heap).with_files(files);
    vm.set_abort_flag(&ABORT);
    vm
}

fn run(vm: &mut Vm, source: &'static str, interactive: bool) -> Result<(), Error> {
    ABORT.store(false, Ordering::Relaxed);
    RUNNING.store(true, Ordering::Relaxed);
    vm.set_interactive(interactive);
    let result = vm.eval(source);
    vm.set_interactive(false);
    RUNNING.store(false, Ordering::Relaxed);
    result
}

/// A Python traceback, like the firmware prints it.
fn report(vm: &mut Vm, result: Result<(), Error>) {
    if let Err(err) = result {
        write("Traceback (most recent call last):\n");
        match vm.error_line() {
            Some(line) => write(&format!("  File \"<stdin>\", line {}, in <module>\n", line)),
            None => write("  File \"<stdin>\", in <module>\n"),
        }
        vm.print_exception(err);
        write("\n");
    }
}

fn banner() {
    write(concat!(
        "MicroPython-compatible micropython-rs ",
        env!("CARGO_PKG_VERSION"),
        " on a PC; Type \"help()\" for more information.\r\n"
    ));
}

pub fn main() -> ! {
    let flash: &'static mut [u8] = Box::leak(vec![0u8; 30 * SLOT_SIZE as usize].into_boxed_slice());
    let fs = FlatFs::mount(RamFlash::new(flash)).expect("RAM file system");
    let files: &'static RefCell<dyn FileSystem> = Box::leak(Box::new(RefCell::new(fs)));
    let input = reader();
    let mut vm = new_vm(files);
    let mut raw = false;
    let mut line: Vec<u8> = Vec::new();
    banner();
    write(">>> ");
    loop {
        let Ok(byte) = input.recv() else {
            std::process::exit(0);
        };
        match byte {
            CTRL_A => {
                raw = true;
                line.clear();
                write("raw REPL; CTRL-B to exit\r\n>");
            }
            CTRL_B => {
                raw = false;
                line.clear();
                write("\r\n");
                banner();
                write(">>> ");
            }
            CTRL_C => {
                line.clear();
                if !raw {
                    write("\r\nKeyboardInterrupt\r\n>>> ");
                }
            }
            CTRL_D if line.is_empty() => {
                // Soft reboot: a new interpreter (files stay)
                vm = new_vm(files);
                if raw {
                    write("OK\r\n"); // like MicroPython: the empty command is acknowledged
                }
                write("MPY: soft reboot\r\n");
                if raw {
                    write("raw REPL; CTRL-B to exit\r\n>");
                } else {
                    banner();
                    write(">>> ");
                }
            }
            CTRL_D if raw => {
                // "OK", the output, \x04, the error text, \x04, ">"
                write("OK");
                let result = match keep(std::mem::take(&mut line)) {
                    Ok(source) => run(&mut vm, source, false),
                    Err(err) => Err(err),
                };
                write("\x04");
                report(&mut vm, result);
                write("\x04>");
            }
            CTRL_E if raw && line.is_empty() => {
                // Raw-paste mode: "understood, but not supported"
                let (a, b) = (input.recv(), input.recv());
                if a == Ok(b'A') && b == Ok(CTRL_A) {
                    write("R\x00");
                }
            }
            b'\r' | b'\n' if !raw => {
                write("\r\n");
                if !line.is_empty() {
                    if let Ok(source) = keep(std::mem::take(&mut line)) {
                        let result = run(&mut vm, source, true);
                        report(&mut vm, result);
                    }
                }
                write(">>> ");
            }
            byte => {
                line.push(byte);
                if !raw {
                    let _ = std::io::stdout().lock().write_all(&[byte]);
                    let _ = std::io::stdout().flush();
                }
            }
        }
    }
}
