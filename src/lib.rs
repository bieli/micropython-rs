#![no_std]
// No `unsafe` code in the interpreter. The `iram` feature's only difference is
// the `link_section` attribute on hot functions, which the lint counts as
// unsafe: every build without it (tests, CI, clippy) proves the rest.
#![cfg_attr(not(feature = "iram"), forbid(unsafe_code))]

pub mod ast_parser;
pub mod calendar;
pub mod flatfs;
pub mod heap;
pub mod interpreter;
pub mod lexer;
pub mod returns;

pub use flatfs::FileSystem;

/// The float type of the interpreter: `f64`, or `f32` with the `float32`
/// feature (Python's `float` is double precision; MicroPython's ESP32 port
/// uses single precision).
#[cfg(not(feature = "float32"))]
pub type Float = f64;
#[cfg(feature = "float32")]
pub type Float = f32;

/// `Float` as `f64` (exact). Without `float32` it's the value itself.
#[allow(clippy::useless_conversion)]
pub(crate) fn float_to_f64(f: Float) -> f64 {
    f64::from(f)
}

/// The bits of a `Float` for a 64-bit container slot, and back.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn float_bits(f: Float) -> u64 {
    f.to_bits() as u64
}

#[allow(clippy::unnecessary_cast)]
pub(crate) fn float_from_bits(bits: u64) -> Float {
    Float::from_bits(bits as _)
}
pub use ast_parser::StmtCache;
pub use interpreter::Interpreter;
pub use interpreter::Value;
pub use returns::{Ret, Returns};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    UnexpectedChar(char),
    UnexpectedToken,
    UnterminatedString,
    InvalidSyntax,
    UnknownIdentifier,
    OutOfMemory,
    /// One statement's expression is too big or too deeply nested for the
    /// parser's fixed buffers (`ast_parser::MAX_EXPR_ITEMS`, `MAX_EXPR_DEPTH`):
    /// a `SyntaxError`, fixed by splitting it into smaller statements.
    TooComplex,
    TooManyArguments,
    /// Operator applied to unsupported operand types (e.g. `'a' + 1`).
    TypeError,
    /// `x / 0`, `x // 0` or `x % 0`.
    ZeroDivision,
    /// Integer result does not fit into `i64`.
    Overflow,
    /// The script executed more statements than `Interpreter::set_step_limit` allows.
    StepLimit,
    /// The abort flag registered with `Interpreter::set_abort_flag` was set.
    Interrupted,
    /// Unexpected indent, an inconsistent dedent, or a missing indented block.
    IndentationError,
    /// Too many nested function calls (see `interpreter::MAX_CALL_DEPTH`).
    RecursionLimit,
    /// Invalid value for an operation, e.g. a negative shift count.
    ValueError,
    /// A hardware operation failed, e.g. an I2C device did not answer.
    IoError,
    /// `import` of a module the bridge doesn't provide, or `from m import x`
    /// of a name the module doesn't have.
    ImportError,
    /// `module.name` for a name the module doesn't have, or `.` on a value
    /// that isn't a module.
    AttributeError,
    /// An index outside a string, bytes, list or tuple.
    IndexError,
    /// A dict has no such key.
    KeyError,
    /// The feature isn't in this build's feature level (e.g. `class` in a
    /// `minimal` build). Python sees a `NotImplementedError`.
    NotEnabled,
    /// A Python exception from `raise` (or an error re-raised by `raise`):
    /// `Interpreter::write_exception` prints its class and message.
    Exception,
    /// From a hardware function that waits (a sleep): it stopped early, with
    /// this many milliseconds left, because events are waiting for their
    /// handlers (`HardwareBridge::poll_event`). The interpreter runs them and
    /// continues with `HardwareBridge::resume_sleep`; scripts never see it.
    Pending(u32),
}

impl Error {
    /// A short description, like the message of the Python exception.
    pub const fn message(&self) -> &'static str {
        match self {
            Error::UnexpectedChar(_) => "invalid character",
            Error::UnexpectedToken | Error::InvalidSyntax => "invalid syntax",
            Error::UnterminatedString => "unterminated string",
            Error::UnknownIdentifier => "name is not defined",
            Error::OutOfMemory => "memory allocation failed",
            Error::TooComplex => "expression too complex: split it into smaller statements",
            Error::TooManyArguments => "too many arguments or names",
            Error::TypeError => "unsupported operand or argument type",
            Error::ZeroDivision => "division by zero",
            Error::Overflow => "integer overflow",
            Error::StepLimit => "step limit reached",
            Error::Interrupted => "",
            Error::IndentationError => "unexpected indent or dedent",
            Error::RecursionLimit => "maximum recursion depth exceeded",
            Error::ValueError => "invalid value",
            Error::IoError => "I/O error",
            Error::ImportError => "no module or name to import",
            Error::AttributeError => "no such attribute",
            Error::IndexError => "index out of range",
            Error::KeyError => "key not found",
            Error::NotEnabled => "not enabled in this build",
            Error::Exception => "",
            Error::Pending(_) => "interrupted by an event",
        }
    }
}

pub trait Output {
    fn write_str(&mut self, s: &str);
}

/// An argument passed to a hardware function.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arg<'s> {
    /// An `int` (bools arrive as 0/1, like in Python).
    Int(i64),
    /// A `float`, e.g. for `time.sleep(0.5)` (as `f64` also with the
    /// `float32` feature: widening is exact).
    Float(f64),
    /// A `str`, e.g. for `text(x, y, 'hello')`.
    Str(&'s str),
    /// A `bytes` value, e.g. for `i2c_write(0x3c, b'\x00\xaf')`.
    Bytes(&'s [u8]),
}

impl<'s> Arg<'s> {
    /// The integer value, or `Error::TypeError` for a string or a float
    /// (like MicroPython: "can't convert float to int").
    pub fn int(self) -> Result<i64, Error> {
        match self {
            Arg::Int(n) => Ok(n),
            _ => Err(Error::TypeError),
        }
    }

    /// The value of a float or an int as `f64`, or `Error::TypeError`.
    pub fn float(self) -> Result<f64, Error> {
        match self {
            Arg::Float(f) => Ok(f),
            Arg::Int(n) => Ok(n as f64),
            _ => Err(Error::TypeError),
        }
    }

    /// The string value, or `Error::TypeError` for an integer or bytes.
    pub fn str(self) -> Result<&'s str, Error> {
        match self {
            Arg::Str(s) => Ok(s),
            _ => Err(Error::TypeError),
        }
    }

    /// The bytes of a `bytes` value, or of a `str` (its UTF-8), or
    /// `Error::TypeError` for an integer: what a bus write sends.
    pub fn bytes(self) -> Result<&'s [u8], Error> {
        match self {
            Arg::Bytes(b) => Ok(b),
            Arg::Str(s) => Ok(s.as_bytes()),
            Arg::Int(_) | Arg::Float(_) => Err(Error::TypeError),
        }
    }
}

/// Describes one hardware function, for argument checks and `help()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HwFn {
    pub name: &'static str,
    pub min_args: u8,
    pub max_args: u8,
    /// Parameter names for `help()`, e.g. `"pin, value"`.
    pub params: &'static str,
    /// One-line description for `help()`.
    pub doc: &'static str,
    /// The function returns `bytes` (see `HardwareBridge::call_bytes`).
    pub returns_bytes: bool,
    /// The function returns any values: floats, strings, tuples, lists,
    /// dicts (see `HardwareBridge::call_values`).
    pub returns_values: bool,
}

impl HwFn {
    /// A function with exactly `params.split(',').count()` arguments
    /// (zero for an empty `params`).
    pub const fn new(name: &'static str, params: &'static str, doc: &'static str) -> Self {
        let n = count_params(params);
        Self::with_args(name, n, n, params, doc)
    }

    /// A function with a variable number of arguments.
    pub const fn with_args(
        name: &'static str,
        min_args: u8,
        max_args: u8,
        params: &'static str,
        doc: &'static str,
    ) -> Self {
        Self {
            name,
            min_args,
            max_args,
            params,
            doc,
            returns_bytes: false,
            returns_values: false,
        }
    }

    /// Marks a function that returns `bytes` (`i2c_read`, `uart_read`, ...):
    /// the interpreter calls `call_bytes` / `call_module_bytes` for it.
    pub const fn returning_bytes(mut self) -> Self {
        self.returns_bytes = true;
        self
    }

    /// Marks a function that returns any values (floats, strings, tuples,
    /// lists, dicts): the interpreter calls `call_values` for it.
    pub const fn returning_values(mut self) -> Self {
        self.returns_values = true;
        self
    }
}

/// Number of comma-separated names in `params` (0 for `""`).
const fn count_params(params: &str) -> u8 {
    let bytes = params.as_bytes();
    if bytes.is_empty() {
        return 0;
    }
    let mut count = 1;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b',' {
            count += 1;
        }
        i += 1;
    }
    count
}

/// A native module, written in Rust and imported by scripts with `import`:
/// `import display` then `display.line(0, 0, 10, 10)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Module {
    pub name: &'static str,
    /// One-line description for `help()`.
    pub doc: &'static str,
    /// Functions, called through `HardwareBridge::call_module`. Argument
    /// counts are checked before the call.
    pub functions: &'static [HwFn],
    /// Integer constants, e.g. `display.WIDTH`.
    pub constants: &'static [(&'static str, i64)],
}

/// Connects scripts to hardware. Implement `call` for integer-only functions,
/// or `call_args` to also receive strings. All methods have defaults, so a
/// bridge implements only what it needs.
pub trait HardwareBridge {
    /// Called for functions whose arguments are all integers.
    fn call(&mut self, _name: &str, _args: &[i64]) -> Result<Option<i64>, Error> {
        Err(Error::UnknownIdentifier)
    }

    /// Called by the interpreter for every hardware function. The default
    /// converts the arguments to integers and calls `call`; override it to
    /// accept string arguments.
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        let mut ints = [0i64; ast_parser::MAX_CALL_ARGS];
        for (slot, arg) in ints.iter_mut().zip(args) {
            *slot = arg.int()?;
        }
        self.call(name, &ints[..args.len()])
    }

    /// The functions this bridge provides. When the list is not empty, the
    /// interpreter rejects unknown names (`UnknownIdentifier`) and wrong
    /// argument counts (`TypeError`) before calling, and `help()` lists them.
    fn functions(&self) -> &[HwFn] {
        &[]
    }

    /// Named integer constants visible to scripts, e.g. `("OUT", 1)`.
    /// Variables with the same name take precedence.
    fn constants(&self) -> &[(&'static str, i64)] {
        &[]
    }

    /// Native modules scripts can `import`.
    fn modules(&self) -> &[Module] {
        &[]
    }

    /// Called for `module.function(args)` (also through an alias or
    /// `from module import function`), with the module's real name. The
    /// function exists in the module's table and the argument count fits.
    fn call_module(
        &mut self,
        _module: &str,
        _name: &str,
        _args: &[Arg<'_>],
    ) -> Result<Option<i64>, Error> {
        Err(Error::UnknownIdentifier)
    }

    /// Calls a function marked with `HwFn::returning_bytes`: write the result
    /// into `out` (up to `out.len()`, `MAX_BRIDGE_BYTES` at most) and return
    /// how many bytes it has. The script gets them as a `bytes` object.
    fn call_bytes(
        &mut self,
        _name: &str,
        _args: &[Arg<'_>],
        _out: &mut [u8],
    ) -> Result<usize, Error> {
        Err(Error::UnknownIdentifier)
    }

    /// The next event waiting for its Python handler, as a number: a pin
    /// interrupt or a timer that fired (the firmware's `machine` module maps
    /// the numbers to `Pin.irq()` and `Timer` callbacks). The interpreter
    /// asks between statements, while it has a handler (`_irq_handler`), and
    /// calls the handler with the number, until this returns `None`. Like
    /// MicroPython's soft interrupts: callbacks run between statements.
    fn poll_event(&mut self) -> Option<i64> {
        None
    }

    /// Whether sleeps may stop early for events (returning
    /// `Error::Pending`): only while the script has a handler and isn't
    /// running one already (handlers don't nest).
    fn set_events(&mut self, _on: bool) {}

    /// The application is idle (a REPL waiting for input; see
    /// `Interpreter::idle`): background work, like serving a network.
    fn idle(&mut self) {}

    /// A soft reset (the application calls it before a new session): the
    /// last session's pin interrupts and timers stop.
    fn soft_reset(&mut self) {}

    /// Continues a sleep that returned `Error::Pending`, after the handlers
    /// ran: `remaining_ms` more (and may stop early again).
    fn resume_sleep(&mut self, _remaining_ms: u32) -> Result<(), Error> {
        Ok(())
    }

    /// `call_bytes` for a function of a native module.
    fn call_module_bytes(
        &mut self,
        _module: &str,
        _name: &str,
        _args: &[Arg<'_>],
        _out: &mut [u8],
    ) -> Result<usize, Error> {
        Err(Error::UnknownIdentifier)
    }

    /// Calls a function marked with `HwFn::returning_values` (`module` is
    /// `None` for a plain function): record what it returns in `out`, any
    /// values, nested (see `returns`). Nothing recorded gives `None`, several
    /// values at the top level a tuple of them.
    ///
    /// ```ignore
    /// "read" => {                     // bme.read() -> (21.37, 1013.2, 45.8)
    ///     out.begin_tuple()?;
    ///     out.float(t)?;
    ///     out.float(p)?;
    ///     out.float(h)?;
    ///     out.end()
    /// }
    /// ```
    fn call_values(
        &mut self,
        _module: Option<&str>,
        _name: &str,
        _args: &[Arg<'_>],
        _out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        Err(Error::UnknownIdentifier)
    }
}

/// Most bytes one bridge call can return (`i2c_read(addr, n)`, ...).
pub const MAX_BRIDGE_BYTES: usize = 512;

pub struct NoHardware;

impl HardwareBridge for NoHardware {}
