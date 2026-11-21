// ============================================================================
// INTERPRETER / EVALUATOR
// ============================================================================

use crate::ast_parser::{Expr, Parser, Stmt};
use crate::{Error, Output};

pub struct Interpreter<'o, O: Output> {
    out: &'o mut O,
}

impl<'o, O: Output> Interpreter<'o, O> {
    pub fn new(out: &'o mut O) -> Self {
        Self { out }
    }

    pub fn eval(&mut self, source: &str) -> Result<(), Error> {
        let mut parser = Parser::new(source)?;
        while let Some(stmt) = parser.parse_next_stmt()? {
            self.exec_stmt(stmt)?;
        }
        Ok(())
    }

    fn exec_stmt(&mut self, stmt: Stmt<'_>) -> Result<(), Error> {
        match stmt {
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
                        self.print_expr(args[i]);
                    }
                    self.out.write_str("\n");
                    Ok(())
                }
                _ => Err(Error::UnknownIdentifier),
            },
        }
    }

    fn print_expr(&mut self, expr: Expr<'_>) {
        match expr {
            Expr::None => self.out.write_str("None"),
            Expr::Str(s) => self.out.write_str(s),
            Expr::Int(n) => self.write_i64(n),
        }
    }

    /// Formats an i64 onto a stack buffer without needing `alloc` or `core::fmt`.
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
    fn test_hello_world() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        let code = r#"
# First micropython-rs program
print('hello world')
print("Answer:", 42)
"#;

        vm.eval(code).unwrap();
        assert_eq!(out.as_str(), "hello world\nAnswer: 42\n");
    }
}
