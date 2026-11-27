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
    // The source of imported `.py` modules (from the RAM file system)
    let mut module_memory = vec![0u8; 256 * 1024];
    // Parsed statements kept for loops and functions (see `with_stmt_cache`)
    let mut stmt_cache = vec![micropython_rs::StmtCache::EMPTY; 64];
    // As many functions as a script may have alive at once (the most there can be)
    let mut func_slots = vec![None; 255];
    let mut vm = Interpreter::new(&mut out)
        .with_heap(&mut heap)
        .with_files(&files)
        .with_module_memory(&mut module_memory)
        .with_stmt_cache(&mut stmt_cache)
        .with_func_slots(&mut func_slots);

    if let Err(err) = vm.eval(&script) {
        // Like Python: the traceback goes to stderr
        vm.write_traceback(err, path, &mut StderrSink);
        eprintln!();
        process::exit(1);
    }
}
