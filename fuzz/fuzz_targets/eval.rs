//! Any text through the whole interpreter: lexer, parser, evaluator, heap and
//! garbage collector, files and `import`. Scripts are untrusted input on a
//! device, so nothing may panic, hang or overflow the native stack.
//!
//! The input is split at NUL bytes into several `eval()` calls on the same
//! interpreter, like lines typed into the REPL (variables, functions,
//! classes, files and modules carry over from one to the next).
#![no_main]

use libfuzzer_sys::fuzz_target;
use micropython_rs::flatfs::{FlatFs, RamFlash, SLOT_SIZE};
use micropython_rs::{Interpreter, Output};
use std::cell::RefCell;

/// Output is formatted (as on a device) but thrown away.
struct Sink(usize);

impl Output for Sink {
    fn write_str(&mut self, s: &str) {
        self.0 = self.0.wrapping_add(s.len());
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let mut out = Sink(0);
    // Small, so out-of-memory paths and the garbage collector run often
    let mut heap = vec![0u8; 8 * 1024];
    let mut flash = vec![0u8; 4 * SLOT_SIZE as usize];
    let files = RefCell::new(FlatFs::mount(RamFlash::new(&mut flash)).unwrap());
    let mut module_memory = vec![0u8; 2 * 1024];
    let mut vm = Interpreter::new(&mut out)
        .with_heap(&mut heap)
        .with_files(&files)
        .with_module_memory(&mut module_memory);
    // `while 1:` must end; no stack floor on purpose: unbounded native
    // recursion has to show up as a crash here, not as a RecursionError
    vm.set_step_limit(Some(20_000));
    vm.set_interactive(true);
    for chunk in text.split('\0') {
        if let Err(err) = vm.eval(chunk) {
            // Rendering the error (with its exception object) must not panic either
            let _ = vm.error_line();
            let mut message = Sink(0);
            vm.write_exception(err, &mut message);
        }
    }
});
