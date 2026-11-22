#![no_std]

pub mod ast_parser;
pub mod interpreter;
pub mod lexer;

pub use interpreter::Interpreter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    UnexpectedChar(char),
    UnexpectedToken,
    UnterminatedString,
    InvalidSyntax,
    UnknownIdentifier,
    OutOfMemory,
    TooManyArguments,
}

pub trait Output {
    fn write_str(&mut self, s: &str);
}

pub trait HardwareBridge {
    fn call(&mut self, name: &str, args: &[i64]) -> Result<Option<i64>, Error>;
}

pub struct NoHardware;

impl HardwareBridge for NoHardware {
    fn call(&mut self, _name: &str, _args: &[i64]) -> Result<Option<i64>, Error> {
        Err(Error::UnknownIdentifier)
    }
}
