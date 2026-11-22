// ============================================================================
// INTERPRETER / EVALUATOR
// ============================================================================

use crate::ast_parser::{Expr, Parser, Stmt};
use crate::{Error, Output};

const MAX_VARS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    None,
    Str(&'a str),
    Int(i64),
}

pub struct Interpreter<'a, 'o, O: Output> {
    out: &'o mut O,
    vars: [Option<(&'a str, Value<'a>)>; MAX_VARS],
}

impl<'a, 'o, O: Output> Interpreter<'a, 'o, O> {
    pub fn new(out: &'o mut O) -> Self {
        Self {
            out,
            vars: [None; MAX_VARS],
        }
    }

    pub fn eval(&mut self, source: &'a str) -> Result<(), Error> {
        let mut parser = Parser::new(source)?;
        while let Some(stmt) = parser.parse_next_stmt()? {
            self.exec_stmt(stmt)?;
        }
        Ok(())
    }

    fn set_var(&mut self, name: &'a str, val: Value<'a>) -> Result<(), Error> {
        // 1. Update existing variable if already defined
        for slot in self.vars.iter_mut().flatten() {
            if slot.0 == name {
                slot.1 = val;
                return Ok(());
            }
        }
        // 2. Otherwise store in the first empty slot
        for slot in self.vars.iter_mut() {
            if slot.is_none() {
                *slot = Some((name, val));
                return Ok(());
            }
        }
        Err(Error::OutOfMemory)
    }

    fn get_var(&self, name: &str) -> Result<Value<'a>, Error> {
        for (var_name, val) in self.vars.iter().flatten() {
            if *var_name == name {
                return Ok(*val);
            }
        }
        Err(Error::UnknownIdentifier)
    }

    fn eval_expr(&self, expr: Expr<'a>) -> Result<Value<'a>, Error> {
        match expr {
            Expr::None => Ok(Value::None),
            Expr::Str(s) => Ok(Value::Str(s)),
            Expr::Int(n) => Ok(Value::Int(n)),
            Expr::Ident(name) => self.get_var(name),
        }
    }

    fn exec_stmt(&mut self, stmt: Stmt<'a>) -> Result<(), Error> {
        match stmt {
            Stmt::Assign { name, value } => {
                let val = self.eval_expr(value)?;
                self.set_var(name, val)
            }
            Stmt::Call {
                func,
                args,
                arg_count,
            } => match func {
                "print" => {
                    for (i, _item) in args.iter().enumerate().take(arg_count) {
                        if i > 0 {
                            self.out.write_str(" ");
                        }
                        let val = self.eval_expr(args[i])?;
                        self.print_value(val);
                    }
                    self.out.write_str("\n");
                    Ok(())
                }
                _ => Err(Error::UnknownIdentifier),
            },
        }
    }

    fn print_value(&mut self, val: Value<'_>) {
        match val {
            Value::None => self.out.write_str("None"),
            Value::Str(s) => self.out.write_str(s),
            Value::Int(n) => self.write_i64(n),
        }
    }

    fn write_i64(&mut self, mut n: i64) {
        if n == 0 {
            self.out.write_str("0");
            return;
        }
        let mut buf = [0u8; 20];
        let mut idx = buf.len();
        let negative = n < 0;
        if negative {
            n = n.wrapping_neg();
        }
        while n > 0 {
            idx -= 1;
            buf[idx] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        if negative {
            idx -= 1;
            buf[idx] = b'-';
        }
        if let Ok(s) = core::str::from_utf8(&buf[idx..]) {
            self.out.write_str(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockOut {
        buf: [u8; 128],
        len: usize,
    }

    impl MockOut {
        const fn new() -> Self {
            Self {
                buf: [0; 128],
                len: 0,
            }
        }
        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.buf[..self.len]).unwrap()
        }
    }

    impl Output for MockOut {
        fn write_str(&mut self, s: &str) {
            let bytes = s.as_bytes();
            self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
    }

    #[test]
    fn test_variables_and_print() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        let code = r#"
msg = 'Hello, Rust!'
x = 10
x = 42
y = x
print(msg)
print('Answer:', y)
"#;

        vm.eval(code).unwrap();
        assert_eq!(out.as_str(), "Hello, Rust!\nAnswer: 42\n");
    }

    #[test]
    fn test_unknown_variable_error() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        assert_eq!(
            vm.eval("print(undefined_var)"),
            Err(Error::UnknownIdentifier)
        );
        assert_eq!(vm.eval("x = missing"), Err(Error::UnknownIdentifier));
    }

    #[test]
    fn test_unknown_function_error() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        assert_eq!(vm.eval("unknown_func(123)"), Err(Error::UnknownIdentifier));
    }

    #[test]
    fn test_out_of_memory_when_exceeding_max_vars() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        // Fill all 32 variable slots (MAX_VARS = 32)
        let mut buf = [0u8; 512];
        let mut len = 0;
        for i in 0..MAX_VARS {
            // Write "v0 = 0\n", "v1 = 1\n", ... into a stack buffer
            let prefix = b"v";
            buf[len..len + prefix.len()].copy_from_slice(prefix);
            len += prefix.len();

            if i >= 10 {
                buf[len] = b'0' + (i / 10) as u8;
                len += 1;
            }
            buf[len] = b'0' + (i % 10) as u8;
            len += 1;

            let suffix = b" = 1\n";
            buf[len..len + suffix.len()].copy_from_slice(suffix);
            len += suffix.len();
        }

        let fill_script = core::str::from_utf8(&buf[..len]).unwrap();
        assert_eq!(vm.eval(fill_script), Ok(()));

        // Overwriting an existing variable still succeeds when full
        assert_eq!(vm.eval("v0 = 99"), Ok(()));

        // Adding a 33rd distinct variable fails with OutOfMemory
        assert_eq!(vm.eval("overflow = 1"), Err(Error::OutOfMemory));
    }

    #[test]
    fn test_print_zero_and_empty_call() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        vm.eval("print(0)\nprint()").unwrap();
        assert_eq!(out.as_str(), "0\n\n");
    }

    #[test]
    fn test_propagates_lexer_and_parser_errors() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        assert_eq!(
            vm.eval("print('unterminated"),
            Err(Error::UnterminatedString)
        );
        assert_eq!(vm.eval("123 = 456"), Err(Error::InvalidSyntax));
        assert_eq!(vm.eval("x = "), Err(Error::UnexpectedToken));
    }
}
