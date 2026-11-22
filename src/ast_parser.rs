// ============================================================================
// AST & PARSER (Recursive Descent, Zero-Allocation)
// ============================================================================
//
// Expressions are compiled into a flat, stack-allocated Reverse Polish
// Notation (RPN) buffer (`Code`) instead of a heap-allocated tree:
//
//     x = (a + 2) * -3      =>   [Ident(a), Int(2), Bin(Add), Int(3), Neg, Bin(Mul)]
//
// Every statement gets one `Code` buffer, and each expression in the statement
// (assignment value or call argument) is an `Expr` range inside that buffer.
//
// Operator precedence (lowest -> highest), matching Python:
//
//     +  -              (left-assoc)
//     *  /  //  %       (left-assoc)
//     unary -  +
//     **                (right-assoc, binds tighter than unary on its left:
//                        -2**2 == -4, 2**-1 is parsed as 2**(-1))
//     atoms: int, 'str', name, ( expr )

use crate::lexer::{Lexer, Token};
use crate::Error;

pub const MAX_CALL_ARGS: usize = 8;
/// Maximum number of RPN items (operands + operators) in one statement.
pub const MAX_EXPR_ITEMS: usize = 32;
/// Maximum nesting of parentheses / unary operators (bounds parser recursion).
pub const MAX_EXPR_DEPTH: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    /// `/`: integer (floor) division, as there is no float support yet.
    Div,
    /// `//`: floor division.
    FloorDiv,
    /// `%`: modulo with Python semantics (result has the sign of the divisor).
    Mod,
    /// `**`: power (non-negative integer exponent).
    Pow,
}

/// One RPN instruction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item<'a> {
    Int(i64),
    Str(&'a str),
    Ident(&'a str),
    /// Unary minus: pops one value, pushes its negation.
    Neg,
    /// Binary operator: pops rhs then lhs, pushes `lhs op rhs`.
    Bin(BinOp),
}

/// A range of RPN items inside a statement's `Code` buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Expr {
    start: u8,
    len: u8,
}

/// Fixed-size RPN buffer holding all expressions of one statement.
#[derive(Debug, Clone, Copy)]
pub struct Code<'a> {
    items: [Item<'a>; MAX_EXPR_ITEMS],
    len: usize,
}

impl<'a> Code<'a> {
    pub const fn new() -> Self {
        Self {
            items: [Item::Int(0); MAX_EXPR_ITEMS],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, item: Item<'a>) -> Result<(), Error> {
        if self.len >= MAX_EXPR_ITEMS {
            return Err(Error::OutOfMemory);
        }
        self.items[self.len] = item;
        self.len += 1;
        Ok(())
    }

    /// RPN items of one expression of the current statement.
    pub fn items(&self, expr: Expr) -> &[Item<'a>] {
        let start = expr.start as usize;
        &self.items[start..start + expr.len as usize]
    }
}

impl Default for Code<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stmt<'a> {
    Assign {
        name: &'a str,
        expr: Expr,
    },
    Call {
        name: &'a str,
        args: [Expr; MAX_CALL_ARGS],
        count: usize,
    },
}

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token<'a>,
    depth: usize,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str) -> Result<Self, Error> {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token()?;
        Ok(Self {
            lexer,
            current,
            depth: 0,
        })
    }

    fn advance(&mut self) -> Result<(), Error> {
        self.current = self.lexer.next_token()?;
        Ok(())
    }

    fn skip_newlines(&mut self) -> Result<(), Error> {
        while self.current == Token::Newline {
            self.advance()?;
        }
        Ok(())
    }

    fn expect_stmt_end(&mut self) -> Result<(), Error> {
        match self.current {
            Token::Newline | Token::Eof => Ok(()),
            _ => Err(Error::UnexpectedToken),
        }
    }

    /// Parses the next statement. Its expressions are written into `code`
    /// (cleared first), which the caller reuses for every statement.
    pub fn parse_next_stmt(&mut self, code: &mut Code<'a>) -> Result<Option<Stmt<'a>>, Error> {
        code.clear();
        self.skip_newlines()?;

        if self.current == Token::Eof {
            return Ok(None);
        }

        let name = match self.current {
            Token::Ident(ident) => ident,
            _ => return Err(Error::InvalidSyntax),
        };
        self.advance()?;

        match self.current {
            // Variable assignment: `x = <expr>`
            Token::Assign => {
                self.advance()?;
                let expr = self.parse_expr(code)?;
                self.expect_stmt_end()?;
                Ok(Some(Stmt::Assign { name, expr }))
            }
            // Function or hardware call: `print(...)`, `led_on()`, `delay_ms(500)`
            Token::LParen => self.parse_call_stmt(name, code),
            _ => Err(Error::UnexpectedToken),
        }
    }

    fn parse_call_stmt(
        &mut self,
        name: &'a str,
        code: &mut Code<'a>,
    ) -> Result<Option<Stmt<'a>>, Error> {
        // Consume '('
        self.advance()?;

        let mut args = [Expr::default(); MAX_CALL_ARGS];
        let mut count = 0;

        if self.current != Token::RParen {
            loop {
                if count >= MAX_CALL_ARGS {
                    return Err(Error::TooManyArguments);
                }

                args[count] = self.parse_expr(code)?;
                count += 1;

                match self.current {
                    Token::Comma => {
                        self.advance()?;
                        // Allow trailing comma before ')'
                        if self.current == Token::RParen {
                            break;
                        }
                    }
                    Token::RParen => break,
                    _ => return Err(Error::UnexpectedToken),
                }
            }
        }

        // Consume ')'
        self.advance()?;
        self.expect_stmt_end()?;

        Ok(Some(Stmt::Call { name, args, count }))
    }

    /// Parses one full expression and returns its range in `code`.
    fn parse_expr(&mut self, code: &mut Code<'a>) -> Result<Expr, Error> {
        let start = code.len;
        self.parse_additive(code)?;
        Ok(Expr {
            start: start as u8,
            len: (code.len - start) as u8,
        })
    }

    /// additive := term (('+' | '-') term)*
    fn parse_additive(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_term(code)?;
        loop {
            let op = match self.current {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => return Ok(()),
            };
            self.advance()?;
            self.parse_term(code)?;
            code.push(Item::Bin(op))?;
        }
    }

    /// term := unary (('*' | '/' | '//' | '%') unary)*
    fn parse_term(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_unary(code)?;
        loop {
            let op = match self.current {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                Token::DoubleSlash => BinOp::FloorDiv,
                Token::Percent => BinOp::Mod,
                _ => return Ok(()),
            };
            self.advance()?;
            self.parse_unary(code)?;
            code.push(Item::Bin(op))?;
        }
    }

    /// unary := ('-' | '+') unary | power
    fn parse_unary(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        match self.current {
            Token::Minus | Token::Plus => {
                let negate = self.current == Token::Minus;
                self.advance()?;
                self.nested(|p| p.parse_unary(code))?;
                if negate {
                    code.push(Item::Neg)?;
                }
                Ok(())
            }
            _ => self.parse_power(code),
        }
    }

    /// power := atom ('**' unary)?
    fn parse_power(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_atom(code)?;
        if self.current == Token::DoubleStar {
            self.advance()?;
            self.nested(|p| p.parse_unary(code))?;
            code.push(Item::Bin(BinOp::Pow))?;
        }
        Ok(())
    }

    /// atom := INT | STR | IDENT | '(' expr ')'
    fn parse_atom(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        let item = match self.current {
            Token::Int(n) => Item::Int(n),
            Token::Str(s) => Item::Str(s),
            Token::Ident(ident) => Item::Ident(ident),
            Token::LParen => {
                self.advance()?;
                self.nested(|p| p.parse_additive(code))?;
                if self.current != Token::RParen {
                    return Err(Error::UnexpectedToken);
                }
                self.advance()?;
                return Ok(());
            }
            _ => return Err(Error::UnexpectedToken),
        };
        self.advance()?;
        code.push(item)
    }

    /// Runs `f` one nesting level deeper, failing instead of overflowing the
    /// native stack on inputs like `((((((...))))))` or `- - - - - 1`.
    fn nested(&mut self, f: impl FnOnce(&mut Self) -> Result<(), Error>) -> Result<(), Error> {
        if self.depth >= MAX_EXPR_DEPTH {
            return Err(Error::OutOfMemory);
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses a single `x = <expr>` statement and returns its RPN items.
    fn rpn<'a>(src: &'a str, code: &mut Code<'a>) -> Result<[Item<'a>; MAX_EXPR_ITEMS], Error> {
        let mut parser = Parser::new(src)?;
        match parser.parse_next_stmt(code)? {
            Some(Stmt::Assign { expr, .. }) => {
                let mut out = [Item::Int(-1); MAX_EXPR_ITEMS];
                let items = code.items(expr);
                out[..items.len()].copy_from_slice(items);
                Ok(out)
            }
            other => panic!("expected assignment, got {:?}", other),
        }
    }

    fn assert_rpn(src: &str, expected: &[Item<'_>]) {
        let mut code = Code::new();
        let items = rpn(src, &mut code).unwrap();
        assert_eq!(&items[..expected.len()], expected, "source: {}", src);
        assert_eq!(code.len, expected.len(), "source: {}", src);
    }

    #[test]
    fn test_parse_assignments() {
        let mut code = Code::new();
        let mut parser = Parser::new("x = 42\nmsg = 'hello'\ny = x\n").unwrap();

        let Some(Stmt::Assign { name, expr }) = parser.parse_next_stmt(&mut code).unwrap() else {
            panic!("expected assignment");
        };
        assert_eq!(name, "x");
        assert_eq!(code.items(expr), &[Item::Int(42)]);

        let Some(Stmt::Assign { name, expr }) = parser.parse_next_stmt(&mut code).unwrap() else {
            panic!("expected assignment");
        };
        assert_eq!(name, "msg");
        assert_eq!(code.items(expr), &[Item::Str("hello")]);

        let Some(Stmt::Assign { name, expr }) = parser.parse_next_stmt(&mut code).unwrap() else {
            panic!("expected assignment");
        };
        assert_eq!(name, "y");
        assert_eq!(code.items(expr), &[Item::Ident("x")]);

        assert_eq!(parser.parse_next_stmt(&mut code).unwrap(), None);
    }

    #[test]
    fn test_parse_empty_and_hardware_calls() {
        let mut code = Code::new();
        let mut parser = Parser::new("led_on()\ndelay_ms(500)\n").unwrap();

        assert_eq!(
            parser.parse_next_stmt(&mut code).unwrap(),
            Some(Stmt::Call {
                name: "led_on",
                args: [Expr::default(); MAX_CALL_ARGS],
                count: 0,
            })
        );

        let Some(Stmt::Call { name, args, count }) = parser.parse_next_stmt(&mut code).unwrap()
        else {
            panic!("expected call");
        };
        assert_eq!((name, count), ("delay_ms", 1));
        assert_eq!(code.items(args[0]), &[Item::Int(500)]);

        assert_eq!(parser.parse_next_stmt(&mut code).unwrap(), None);
    }

    #[test]
    fn test_parse_multi_arg_call_and_trailing_comma() {
        let mut code = Code::new();
        let mut parser = Parser::new("print('x =', x, 10,)\n").unwrap();

        let Some(Stmt::Call { name, args, count }) = parser.parse_next_stmt(&mut code).unwrap()
        else {
            panic!("expected call");
        };
        assert_eq!((name, count), ("print", 3));
        assert_eq!(code.items(args[0]), &[Item::Str("x =")]);
        assert_eq!(code.items(args[1]), &[Item::Ident("x")]);
        assert_eq!(code.items(args[2]), &[Item::Int(10)]);
    }

    #[test]
    fn test_parse_call_args_with_expressions() {
        let mut code = Code::new();
        let mut parser = Parser::new("delay_ms(t * 2, -1)").unwrap();

        let Some(Stmt::Call { args, count, .. }) = parser.parse_next_stmt(&mut code).unwrap()
        else {
            panic!("expected call");
        };
        assert_eq!(count, 2);
        assert_eq!(
            code.items(args[0]),
            &[Item::Ident("t"), Item::Int(2), Item::Bin(BinOp::Mul)]
        );
        assert_eq!(code.items(args[1]), &[Item::Int(1), Item::Neg]);
    }

    #[test]
    fn test_parse_precedence_and_associativity() {
        use BinOp::*;
        use Item::{Bin, Ident, Int, Neg};

        assert_rpn(
            "x = 1 + 2 * 3",
            &[Int(1), Int(2), Int(3), Bin(Mul), Bin(Add)],
        );
        assert_rpn(
            "x = (1 + 2) * 3",
            &[Int(1), Int(2), Bin(Add), Int(3), Bin(Mul)],
        );
        // Left associativity: 10 - 4 - 3 == (10 - 4) - 3
        assert_rpn(
            "x = 10 - 4 - 3",
            &[Int(10), Int(4), Bin(Sub), Int(3), Bin(Sub)],
        );
        assert_rpn(
            "x = a // 2 % 3 / 4",
            &[
                Ident("a"),
                Int(2),
                Bin(FloorDiv),
                Int(3),
                Bin(Mod),
                Int(4),
                Bin(Div),
            ],
        );
        // Right associativity: 2 ** 3 ** 2 == 2 ** (3 ** 2)
        assert_rpn(
            "x = 2 ** 3 ** 2",
            &[Int(2), Int(3), Int(2), Bin(Pow), Bin(Pow)],
        );
        // ** binds tighter than unary minus on its left: -2 ** 2 == -(2 ** 2)
        assert_rpn("x = -2 ** 2", &[Int(2), Int(2), Bin(Pow), Neg]);
        // ...but accepts a unary operand on its right: 2 ** -1
        assert_rpn("x = 2 ** -1", &[Int(2), Int(1), Neg, Bin(Pow)]);
        // Unary binds tighter than *: -a * 3 == (-a) * 3, unary + is a no-op
        assert_rpn("x = -a * +3", &[Ident("a"), Neg, Int(3), Bin(Mul)]);
        assert_rpn("x = --5", &[Int(5), Neg, Neg]);
    }

    #[test]
    fn test_parse_errors() {
        let mut code = Code::new();

        let mut p1 = Parser::new("123 = 456").unwrap();
        assert_eq!(p1.parse_next_stmt(&mut code), Err(Error::InvalidSyntax));

        let mut p2 = Parser::new("x = ").unwrap();
        assert_eq!(p2.parse_next_stmt(&mut code), Err(Error::UnexpectedToken));

        let mut p3 = Parser::new("led_on(1 2)").unwrap();
        assert_eq!(p3.parse_next_stmt(&mut code), Err(Error::UnexpectedToken));

        let mut p4 = Parser::new("f(1, 2, 3, 4, 5, 6, 7, 8, 9)").unwrap();
        assert_eq!(p4.parse_next_stmt(&mut code), Err(Error::TooManyArguments));
    }

    #[test]
    fn test_parse_arithmetic_errors() {
        let mut code = Code::new();
        for src in [
            "x = 1 +",
            "x = (1 + 2",
            "x = 1 + * 2",
            "x = ()",
            "x = 2 **",
            "x = 1 2",
        ] {
            let mut parser = Parser::new(src).unwrap();
            assert_eq!(
                parser.parse_next_stmt(&mut code),
                Err(Error::UnexpectedToken),
                "source: {}",
                src
            );
        }
    }

    #[test]
    fn test_parse_limits() {
        let mut code = Code::new();

        // Too deeply nested parentheses
        let mut parser = Parser::new("x = ((((((((((((((((((1))))))))))))))))))").unwrap();
        assert_eq!(parser.parse_next_stmt(&mut code), Err(Error::OutOfMemory));

        // Too many RPN items in one statement (33 operands + 32 operators)
        let long = "x = 1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1";
        let mut parser = Parser::new(long).unwrap();
        assert_eq!(parser.parse_next_stmt(&mut code), Err(Error::OutOfMemory));

        // The buffer is reused per statement, so many statements are fine
        let mut parser =
            Parser::new("a = 1 + 2 + 3 + 4 + 5 + 6 + 7 + 8\nb = a * a * a * a * a * a * a\n")
                .unwrap();
        assert!(parser.parse_next_stmt(&mut code).unwrap().is_some());
        assert!(parser.parse_next_stmt(&mut code).unwrap().is_some());
        assert_eq!(parser.parse_next_stmt(&mut code).unwrap(), None);
    }
}
