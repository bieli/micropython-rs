#![no_std]

pub mod ast_parser;
pub mod interpreter;
pub mod lexer;

pub use interpreter::Interpreter;
pub use interpreter::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    UnexpectedChar(char),
    UnexpectedToken,
    UnterminatedString,
    InvalidSyntax,
    UnknownIdentifier,
    OutOfMemory,
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
}

pub trait Output {
    fn write_str(&mut self, s: &str);
}

/// An argument passed to a hardware function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg<'s> {
    /// An `int` (bools arrive as 0/1, like in Python).
    Int(i64),
    /// A `str`, e.g. for `text(x, y, 'hello')`.
    Str(&'s str),
}

impl<'s> Arg<'s> {
    /// The integer value, or `Error::TypeError` for a string.
    pub fn int(self) -> Result<i64, Error> {
        match self {
            Arg::Int(n) => Ok(n),
            Arg::Str(_) => Err(Error::TypeError),
        }
    }

    /// The string value, or `Error::TypeError` for an integer.
    pub fn str(self) -> Result<&'s str, Error> {
        match self {
            Arg::Str(s) => Ok(s),
            Arg::Int(_) => Err(Error::TypeError),
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
        }
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
}

pub struct NoHardware;

impl HardwareBridge for NoHardware {}
