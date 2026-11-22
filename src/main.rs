use micropython_rs::{Interpreter, Output};
use std::{env, fs, process};

struct StdoutSink;

impl Output for StdoutSink {
    fn write_str(&mut self, s: &str) {
        print!("{}", s);
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
    let mut vm = Interpreter::new(&mut out);

    if let Err(err) = vm.eval(&script) {
        match vm.error_line() {
            Some(line) => eprintln!("{}:{}: Error: {:?}", path, line, err),
            None => eprintln!("{}: Error: {:?}", path, err),
        }
        process::exit(1);
    }
}
