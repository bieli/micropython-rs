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
//     atoms: int, 'str', name, name(args...), ( expr )
//
// Calls are expressions too: `f(a, b + 1)` compiles to
// [Ident(a), Ident(b), Int(1), Bin(Add), Call { name: "f", argc: 2 }].

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
    /// Function call: pops `argc` arguments, pushes the return value.
    Call {
        name: &'a str,
        argc: u8,
    },
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
    /// `name = <expr>`
    Assign { name: &'a str, expr: Expr },
    /// Expression statement, evaluated for its side effects: `print(x)`, `led_on()`
    Expr(Expr),
}

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token<'a>,
    /// Line where `current` starts.
    current_line: u32,
    /// Line where the most recently parsed statement starts.
    stmt_line: u32,
    depth: usize,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str) -> Result<Self, Error> {
        let mut lexer = Lexer::new(source);
        // Line numbers of errors in the very first token are reported as line 1
        let current = lexer.next_token()?;
        let current_line = lexer.token_line();
        Ok(Self {
            lexer,
            current,
            current_line,
            stmt_line: current_line,
            depth: 0,
        })
    }

    /// Line (1-based) of the token the parser is looking at. After a parse
    /// or lexer error, this is the line where the problem was found.
    pub const fn line(&self) -> u32 {
        self.current_line
    }

    /// Line (1-based) where the most recently parsed statement starts.
    /// Used to report runtime errors.
    pub const fn stmt_line(&self) -> u32 {
        self.stmt_line
    }

    fn advance(&mut self) -> Result<(), Error> {
        let result = self.lexer.next_token();
        self.current_line = self.lexer.token_line();
        self.current = result?;
        Ok(())
    }

    /// Looks at the token after `current` without consuming anything.
    fn peek(&self) -> Result<Token<'a>, Error> {
        self.lexer.clone().next_token()
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
        self.stmt_line = self.current_line;

        if self.current == Token::Eof {
            return Ok(None);
        }

        let stmt = match self.current {
            // Variable assignment: `x = <expr>`
            Token::Ident(name) if self.peek()? == Token::Assign => {
                self.advance()?; // name
                self.advance()?; // '='
                let expr = self.parse_expr(code)?;
                Stmt::Assign { name, expr }
            }
            // `123 = 456`, `'a' = 1`: assignment to something that is not a name
            _ if self.peek()? == Token::Assign => return Err(Error::InvalidSyntax),
            // Expression statement: `print(x)`, `led_on()`, `delay_ms(t * 2)`
            _ => Stmt::Expr(self.parse_expr(code)?),
        };

        self.expect_stmt_end()?;
        Ok(Some(stmt))
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

    /// atom := INT | STR | IDENT | call | '(' expr ')'
    fn parse_atom(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        let item = match self.current {
            Token::Int(n) => Item::Int(n),
            Token::Str(s) => Item::Str(s),
            Token::Ident(name) => {
                self.advance()?;
                if self.current == Token::LParen {
                    return self.nested(|p| p.parse_call(name, code));
                }
                return code.push(Item::Ident(name));
            }
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

    /// call := IDENT '(' [expr (',' expr)* [',']] ')'
    ///
    /// Arguments are emitted first, then `Call { name, argc }`.
    fn parse_call(&mut self, name: &'a str, code: &mut Code<'a>) -> Result<(), Error> {
        // Consume '('
        self.advance()?;

        let mut argc = 0;
        while self.current != Token::RParen {
            if argc >= MAX_CALL_ARGS {
                return Err(Error::TooManyArguments);
            }
            self.parse_additive(code)?;
            argc += 1;

            match self.current {
                // A trailing comma before ')' is allowed
                Token::Comma => self.advance()?,
                Token::RParen => break,
                _ => return Err(Error::UnexpectedToken),
            }
        }

        // Consume ')'
        self.advance()?;
        code.push(Item::Call {
            name,
            argc: argc as u8,
        })
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

    /// Parses one expression statement and returns its RPN items.
    fn expr_stmt<'a>(parser: &mut Parser<'a>, code: &mut Code<'a>) -> [Item<'a>; MAX_EXPR_ITEMS] {
        let Some(Stmt::Expr(expr)) = parser.parse_next_stmt(code).unwrap() else {
            panic!("expected expression statement");
        };
        let mut out = [Item::Int(-1); MAX_EXPR_ITEMS];
        out[..expr_len(code, expr)].copy_from_slice(code.items(expr));
        out
    }

    fn expr_len(code: &Code<'_>, expr: Expr) -> usize {
        code.items(expr).len()
    }

    #[test]
    fn test_parse_empty_and_hardware_calls() {
        use Item::{Call, Int};
        let mut code = Code::new();
        let mut parser = Parser::new("led_on()\ndelay_ms(500)\n").unwrap();

        let items = expr_stmt(&mut parser, &mut code);
        assert_eq!(code.len, 1);
        assert_eq!(
            items[0],
            Call {
                name: "led_on",
                argc: 0
            }
        );

        let items = expr_stmt(&mut parser, &mut code);
        assert_eq!(
            &items[..code.len],
            &[
                Int(500),
                Call {
                    name: "delay_ms",
                    argc: 1
                }
            ]
        );

        assert_eq!(parser.parse_next_stmt(&mut code).unwrap(), None);
    }

    #[test]
    fn test_parse_multi_arg_call_and_trailing_comma() {
        use Item::{Call, Ident, Int, Str};
        let mut code = Code::new();
        let mut parser = Parser::new("print('x =', x, 10,)\n").unwrap();

        let items = expr_stmt(&mut parser, &mut code);
        assert_eq!(
            &items[..code.len],
            &[
                Str("x ="),
                Ident("x"),
                Int(10),
                Call {
                    name: "print",
                    argc: 3
                }
            ]
        );
    }

    #[test]
    fn test_parse_call_args_with_expressions() {
        use Item::{Bin, Call, Ident, Int, Neg};
        let mut code = Code::new();
        let mut parser = Parser::new("delay_ms(t * 2, -1)").unwrap();

        let items = expr_stmt(&mut parser, &mut code);
        assert_eq!(
            &items[..code.len],
            &[
                Ident("t"),
                Int(2),
                Bin(BinOp::Mul),
                Int(1),
                Neg,
                Call {
                    name: "delay_ms",
                    argc: 2
                },
            ]
        );
    }

    #[test]
    fn test_parse_calls_inside_expressions() {
        use BinOp::{Add, Mul};
        use Item::{Bin, Call, Ident, Int};

        // A call is an operand like any other, and calls can nest
        assert_rpn(
            "x = adc_read(34) * 3 + 1",
            &[
                Int(34),
                Call {
                    name: "adc_read",
                    argc: 1,
                },
                Int(3),
                Bin(Mul),
                Int(1),
                Bin(Add),
            ],
        );
        assert_rpn(
            "x = f(g(a), 2 + h())",
            &[
                Ident("a"),
                Call { name: "g", argc: 1 },
                Int(2),
                Call { name: "h", argc: 0 },
                Bin(Add),
                Call { name: "f", argc: 2 },
            ],
        );
        // A bare name stays a variable read
        assert_rpn("x = millis", &[Ident("millis")]);
    }

    #[test]
    fn test_parse_expression_statements() {
        use Item::{Bin, Int};
        let mut code = Code::new();
        // Any expression is a valid statement, as in Python
        let mut parser = Parser::new("1 + 2\n").unwrap();
        let items = expr_stmt(&mut parser, &mut code);
        assert_eq!(&items[..code.len], &[Int(1), Int(2), Bin(BinOp::Add)]);
    }

    #[test]
    fn test_parse_line_numbers() {
        let mut code = Code::new();
        let mut parser = Parser::new("\n# comment\nx = 1\n\n  print(x)\n").unwrap();

        parser.parse_next_stmt(&mut code).unwrap();
        assert_eq!(parser.stmt_line(), 3);
        parser.parse_next_stmt(&mut code).unwrap();
        assert_eq!(parser.stmt_line(), 5);

        // Errors point at the offending token
        let mut parser = Parser::new("x = 1\ny = (2 +\n").unwrap();
        parser.parse_next_stmt(&mut code).unwrap();
        assert_eq!(
            parser.parse_next_stmt(&mut code),
            Err(Error::UnexpectedToken)
        );
        assert_eq!(parser.line(), 2);

        let mut parser = Parser::new("x = 1\n\nprint('open").unwrap();
        parser.parse_next_stmt(&mut code).unwrap();
        assert_eq!(
            parser.parse_next_stmt(&mut code),
            Err(Error::UnterminatedString)
        );
        assert_eq!(parser.line(), 3);
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

        let mut p5 = Parser::new("'a' = 1").unwrap();
        assert_eq!(p5.parse_next_stmt(&mut code), Err(Error::InvalidSyntax));

        for src in ["f(,)", "f(1,,)", "f(1", "x = 1 y", "f()()"] {
            let mut parser = Parser::new(src).unwrap();
            assert_eq!(
                parser.parse_next_stmt(&mut code),
                Err(Error::UnexpectedToken),
                "source: {}",
                src
            );
        }

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
