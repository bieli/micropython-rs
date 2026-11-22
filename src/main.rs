use micropython_rs::{Interpreter, Output};

struct StdoutSink;

impl Output for StdoutSink {
    fn write_str(&mut self, s: &str) {
        print!("{}", s);
    }
}

fn main() {
    let script = "print('hello world')";
    let mut out = StdoutSink;
    let mut vm = Interpreter::new(&mut out);

    if let Err(err) = vm.eval(script) {
        eprintln!("Runtime Error: {:?}", err);
    }
}
