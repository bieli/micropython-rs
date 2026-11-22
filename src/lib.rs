#![no_std]

pub mod ast_parser;
pub mod interpreter;
pub mod lexer;

pub use interpreter::Interpreter;

/// Hardware/Host abstraction for `print()`.
/// Implement this trait for UART, RTT, VGA buffer, or std::io::Stdout.
pub trait Output {
    fn write_str(&mut self, s: &str);
}

#[derive(Debug, PartialEq)]
pub enum Error {
    UnexpectedChar(char),
    UnexpectedToken,
    UnterminatedString,
    UnknownIdentifier,
    InvalidSyntax,
    TooManyArgs,
}
