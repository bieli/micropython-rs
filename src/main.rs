mod host_repl;

use micropython_rs::flatfs::{FlatFs, RamFlash, SLOT_SIZE};
use micropython_rs::{Interpreter, Output};
use std::cell::RefCell;
use std::{env, fs, process};

struct StdoutSink;

impl Output for StdoutSink {
    fn write_str(&mut self, s: &str) {
        print!("{}", s);
    }
}

struct StderrSink;

impl Output for StderrSink {
    fn write_str(&mut self, s: &str) {
        eprint!("{}", s);
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() == 2 && args[1] == "--repl" {
        host_repl::main();
    }

    if args.len() != 2 {
        eprintln!(
            "Usage: {} <script.py>   (or --repl for a REPL on stdin/stdout)",
            args[0]
        );
        process::exit(1);
    }

    let path = &args[1];
    let script = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) => {
            eprintln!("Error reading '{}': {}", path, err);
            process::exit(1);
        }
    };

    let mut out = StdoutSink;
    // Objects created at runtime (strings, lists, ...) live here
    let mut heap = vec![0u8; 256 * 1024];
    // `open()` and `os` work on a file system in RAM (gone when the script ends),
    // with as many files as the ESP32's flash partition holds
    let mut flash = vec![0u8; 30 * SLOT_SIZE as usize];
    let files = RefCell::new(FlatFs::mount(RamFlash::new(&mut flash)).expect("RAM file system"));
    let mut vm = Interpreter::new(&mut out)
        .with_heap(&mut heap)
        .with_files(&files);

    if let Err(err) = vm.eval(&script) {
        // Like Python: the traceback goes to stderr
        eprintln!("Traceback (most recent call last):");
        match vm.error_line() {
            Some(line) => eprintln!("  File \"{}\", line {}, in <module>", path, line),
            None => eprintln!("  File \"{}\", in <module>", path),
        }
        vm.write_exception(err, &mut StderrSink);
        eprintln!();
        process::exit(1);
    }
}
