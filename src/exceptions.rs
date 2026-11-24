// ============================================================================
// EXCEPTIONS: the built-in exception classes, exception objects, `raise`,
// and matching `except` clauses
// ============================================================================
//
// Errors travel through the interpreter as `Err(Error)`. Internal errors
// (`Error::ZeroDivision`, ...) map to Python exception classes; `raise`
// stores the exception object in `Interpreter::exc` (a GC root) and returns
// `Err(Error::Exception)`.
//
// Exception classes are numbers into `CLASSES`, with CPython's hierarchy.
// An exception object is a heap object laid out like a tuple: its class
// number, then its arguments (`ValueError('bad')` has one).
//
// Not catchable: syntax errors (the parser found them, like Python's
// compile-time errors) and the step limit. `KeyboardInterrupt` (Ctrl-C) is
// only caught by naming it, not by a bare `except:`, so a running device can
// always be stopped.

use super::{Interpreter, Value};
use crate::heap::{HeapView, Kind};
use crate::{Error, HardwareBridge, Output};

/// `(name, parent class)`; the parent of `BaseException` is itself.
const CLASSES: &[(&str, u8)] = &[
    ("BaseException", BASE),
    ("Exception", BASE),
    ("ArithmeticError", EXCEPTION),
    ("ZeroDivisionError", 2),
    ("OverflowError", 2),
    ("LookupError", EXCEPTION),
    ("IndexError", 5),
    ("KeyError", 5),
    ("ValueError", EXCEPTION),
    ("UnicodeError", 8),
    ("TypeError", EXCEPTION),
    ("NameError", EXCEPTION),
    ("AttributeError", EXCEPTION),
    ("ImportError", EXCEPTION),
    ("OSError", EXCEPTION),
    ("MemoryError", EXCEPTION),
    ("RuntimeError", EXCEPTION),
    ("RecursionError", 16),
    ("NotImplementedError", 16),
    ("AssertionError", EXCEPTION),
    ("StopIteration", EXCEPTION),
    ("KeyboardInterrupt", BASE),
    ("SystemExit", BASE),
    ("SyntaxError", EXCEPTION),
    ("IndentationError", 23),
];

const BASE: u8 = 0;
const EXCEPTION: u8 = 1;
const OS_ERROR: u8 = 14;
const RUNTIME_ERROR: u8 = 16;
const ASSERTION_ERROR: u8 = 19;
const KEYBOARD_INTERRUPT: u8 = 21;

pub(crate) fn class_name(class: u8) -> &'static str {
    CLASSES.get(class as usize).map_or("Exception", |c| c.0)
}

/// True for `OSError` and its subclasses (printed as `[Errno N] message`).
pub(crate) fn is_os_error(class: u8) -> bool {
    is_subclass(class, OS_ERROR)
}

pub(super) fn class_by_name(name: &str) -> Option<u8> {
    CLASSES.iter().position(|c| c.0 == name).map(|i| i as u8)
}

/// True if `class` is `of` or derives from it.
fn is_subclass(mut class: u8, of: u8) -> bool {
    loop {
        if class == of {
            return true;
        }
        if class == BASE {
            return false;
        }
        class = CLASSES[class as usize].1;
    }
}

/// The class of an internal error.
fn error_class(err: Error) -> u8 {
    let name = match err {
        Error::UnexpectedChar(_)
        | Error::UnexpectedToken
        | Error::UnterminatedString
        | Error::InvalidSyntax
        | Error::TooManyArguments => "SyntaxError",
        Error::IndentationError => "IndentationError",
        Error::UnknownIdentifier => "NameError",
        Error::OutOfMemory => "MemoryError",
        Error::TypeError => "TypeError",
        Error::ZeroDivision => "ZeroDivisionError",
        Error::Overflow => "OverflowError",
        Error::StepLimit | Error::Exception => "RuntimeError",
        Error::Interrupted => "KeyboardInterrupt",
        Error::RecursionLimit => "RecursionError",
        Error::ValueError => "ValueError",
        Error::IoError => "OSError",
        Error::ImportError => "ImportError",
        Error::AttributeError => "AttributeError",
        Error::IndexError => "IndexError",
        Error::KeyError => "KeyError",
    };
    class_by_name(name).unwrap_or(RUNTIME_ERROR)
}

/// Errors that `try` can't catch.
fn uncatchable(err: Error) -> bool {
    matches!(
        err,
        Error::UnexpectedChar(_)
            | Error::UnexpectedToken
            | Error::UnterminatedString
            | Error::InvalidSyntax
            | Error::TooManyArguments
            | Error::IndentationError
            | Error::StepLimit
    )
}

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    /// The class of the error being handled, or `None` if `try` can't catch it.
    pub(super) fn caught_class(&self, err: Error) -> Option<u8> {
        if uncatchable(err) {
            return None;
        }
        if err == Error::Exception {
            return self.exc_class(self.exc);
        }
        Some(error_class(err))
    }

    /// The class of an exception object.
    pub(super) fn exc_class(&self, v: Value<'a>) -> Option<u8> {
        match v {
            Value::Obj(r) if self.heap.kind(r) == Kind::Exception => match self.slot(r, 0) {
                Value::Int(c) => Some(c as u8),
                // An exception object of a user class: its built-in root class
                Value::Obj(class) => self.exception_root(class),
                _ => None,
            },
            _ => None,
        }
    }

    /// A new exception object of `class` with `args` (rooted).
    pub(super) fn new_exception(
        &mut self,
        class: u8,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        let base = self.sp;
        let result = (|| {
            self.push(Value::Int(class as i64))?;
            for &a in args {
                self.push(a)?;
            }
            self.build(Kind::Tuple, args.len() + 1)?;
            let v = self.stack[self.sp - 1];
            if let Value::Obj(r) = v {
                self.heap.set_kind(r, Kind::Exception);
            }
            Ok(v)
        })();
        self.sp = base;
        result
    }

    /// `raise v`: an exception object, or a class (instantiated without
    /// arguments). Returns the error to propagate.
    pub(super) fn raise_value(&mut self, v: Value<'a>) -> Error {
        let exc = match v {
            Value::ExcClass(class) => match self.new_exception(class, &[]) {
                Ok(exc) => exc,
                Err(err) => return err,
            },
            _ if self.exc_class(v).is_some() => v,
            Value::Obj(r)
                if self.heap.kind(r) == Kind::Class && self.exception_root(r).is_some() =>
            {
                match self.instantiate(r, &[], &[]) {
                    Ok(exc) => exc,
                    Err(err) => return err,
                }
            }
            // `raise 5`: exceptions must derive from BaseException
            _ => return Error::TypeError,
        };
        self.exc = exc;
        Error::Exception
    }

    /// `raise` with an exception class number and a message.
    pub(super) fn raise_new(&mut self, class: u8, message: Option<Value<'a>>) -> Error {
        let args = message.as_slice();
        match self.new_exception(class, args) {
            Ok(exc) => {
                self.exc = exc;
                Error::Exception
            }
            Err(err) => err,
        }
    }

    pub(super) fn raise_assertion(&mut self, message: Option<Value<'a>>) -> Error {
        self.raise_new(ASSERTION_ERROR, message)
    }

    /// A bare `raise`: re-raises the exception being handled.
    pub(super) fn reraise(&mut self) -> Error {
        if self.handling == Value::None {
            let msg = Value::Str("No active exception to reraise");
            return self.raise_new(RUNTIME_ERROR, Some(msg));
        }
        self.exc = self.handling;
        Error::Exception
    }

    /// The exception object for a caught error (made for internal errors,
    /// with their message as the argument).
    pub(super) fn exception_object(&mut self, err: Error) -> Result<Value<'a>, Error> {
        if err == Error::Exception && self.exc_class(self.exc).is_some() {
            return Ok(self.exc);
        }
        let message = err.message();
        let args = [Value::Str(message)];
        let args: &[Value<'a>] = if message.is_empty() { &[] } else { &args };
        self.new_exception(error_class(err), args)
    }

    /// Does an exception of `class` match the `except` specification `spec`
    /// (a class or a tuple of classes)?
    pub(super) fn exc_matches(&self, class: u8, spec: Value<'a>) -> Result<bool, Error> {
        match spec {
            Value::ExcClass(of) => Ok(is_subclass(class, of)),
            // `except MyError`: the pending exception's own class must derive from it
            Value::Obj(r) if self.heap.kind(r) == Kind::Class => Ok(self
                .class_of(self.exc)
                .is_some_and(|c| self.derives_from(c, r))),
            Value::Obj(r) if self.heap.kind(r) == Kind::Tuple => {
                for i in 0..self.count(r) {
                    if self.exc_matches(class, self.slot(r, i))? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Err(Error::TypeError),
        }
    }

    /// A bare `except:` catches everything but `KeyboardInterrupt`.
    pub(super) fn bare_except_catches(class: u8) -> bool {
        class != KEYBOARD_INTERRUPT
    }

    /// `exc.args` (a tuple) and `exc.errno` (of an `OSError`).
    pub(super) fn exc_attr(&mut self, exc: Value<'a>, name: &str) -> Result<Value<'a>, Error> {
        let (Value::Obj(r), Some(class)) = (exc, self.exc_class(exc)) else {
            return Err(Error::AttributeError);
        };
        let n = self.count(r) - 1;
        match name {
            "args" => {
                let base = self.sp;
                let result = (|| {
                    for i in 0..n {
                        self.push(self.slot(r, i + 1))?;
                    }
                    self.build(Kind::Tuple, n)?;
                    Ok(self.stack[self.sp - 1])
                })();
                self.sp = base;
                result
            }
            "errno" if is_subclass(class, OS_ERROR) => {
                Ok(if n > 0 { self.slot(r, 1) } else { Value::None })
            }
            "value" if n > 0 => Ok(self.slot(r, 1)),
            _ => Err(Error::AttributeError),
        }
    }

    /// Writes the last line of a Python traceback for `err` to `out`:
    /// `ValueError: bad value` (or just the class name).
    pub fn write_exception(&self, err: Error, out: &mut dyn Output) {
        write_exception(&self.heap.view(), self.hw_modules(), self.exc, err, out);
    }

    /// Like `write_exception`, to the interpreter's own `Output` (e.g. for
    /// a REPL, after it printed `Traceback (most recent call last):`).
    pub fn print_exception(&mut self, err: Error) {
        let hw_modules: &[crate::Module] = match &self.hw {
            Some(hw) => hw.modules(),
            None => &[],
        };
        write_exception(&self.heap.view(), hw_modules, self.exc, err, &mut *self.out);
    }
}

fn write_exception(
    view: &HeapView<'_>,
    hw_modules: &[crate::Module],
    exc: Value<'_>,
    err: Error,
    out: &mut dyn Output,
) {
    let class = match (err, exc) {
        (Error::Exception, Value::Obj(r)) if view.kind(r) == Kind::Exception => {
            match super::containers::slot(view, r, 0) {
                Value::Int(c) => Some((c as u8, r)),
                Value::Obj(_) => Some((1, r)),
                _ => None,
            }
        }
        _ => None,
    };
    let Some((class, r)) = class else {
        out.write_str(class_name(error_class(err)));
        if !err.message().is_empty() {
            out.write_str(": ");
            out.write_str(err.message());
        }
        return;
    };
    match super::containers::slot(view, r, 0) {
        Value::Obj(user) => out.write_str(view.str(match super::containers::slot(view, user, 0) {
            Value::HStr(name) => name,
            _ => user,
        })),
        _ => out.write_str(class_name(class)),
    }
    if super::containers::count(view, r) > 1 {
        out.write_str(": ");
        let lookup = |i: u8| super::lookup_module(hw_modules, i);
        super::containers::render(view, &lookup, exc, false, 0, &mut |p| out.write_str(p));
    }
}
