use micropython_rs::{Interpreter, Output};
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

    if args.len() != 2 {
        eprintln!("Usage: {} <script.py>", args[0]);
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
    let mut vm = Interpreter::new(&mut out).with_heap(&mut heap);

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
