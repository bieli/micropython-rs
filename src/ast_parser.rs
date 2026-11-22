// ============================================================================
// AST & PARSER (Recursive Descent, Zero-Allocation)
// ============================================================================

use crate::lexer::{Lexer, Token};
use crate::Error;

pub const MAX_CALL_ARGS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Expr<'a> {
    None,
    Str(&'a str),
    Int(i64),
    Ident(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stmt<'a> {
    Assign {
        name: &'a str,
        expr: Expr<'a>,
    },
    Call {
        name: &'a str,
        args: [Option<Expr<'a>>; MAX_CALL_ARGS],
        count: usize,
    },
}

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token<'a>,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str) -> Result<Self, Error> {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token()?;
        Ok(Self { lexer, current })
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

    pub fn parse_next_stmt(&mut self) -> Result<Option<Stmt<'a>>, Error> {
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
                let expr = self.parse_expr()?;
                self.expect_stmt_end()?;
                Ok(Some(Stmt::Assign { name, expr }))
            }
            // Function or hardware call: `print(...)`, `led_on()`, `delay_ms(500)`
            Token::LParen => self.parse_call_stmt(name),
            _ => Err(Error::UnexpectedToken),
        }
    }

    fn parse_call_stmt(&mut self, name: &'a str) -> Result<Option<Stmt<'a>>, Error> {
        // Consume '('
        self.advance()?;

        let mut args = [None; MAX_CALL_ARGS];
        let mut count = 0;

        if self.current != Token::RParen {
            loop {
                if count >= MAX_CALL_ARGS {
                    return Err(Error::TooManyArguments);
                }

                args[count] = Some(self.parse_expr()?);
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

    fn parse_expr(&mut self) -> Result<Expr<'a>, Error> {
        let expr = match self.current {
            Token::Int(n) => Expr::Int(n),
            Token::Str(s) => Expr::Str(s),
            Token::Ident(ident) => Expr::Ident(ident),
            _ => return Err(Error::UnexpectedToken),
        };
        self.advance()?;
        Ok(expr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_assignments() {
        let mut parser = Parser::new("x = 42\nmsg = 'hello'\ny = x\n").unwrap();

        assert_eq!(
            parser.parse_next_stmt().unwrap(),
            Some(Stmt::Assign {
                name: "x",
                expr: Expr::Int(42),
            })
        );
        assert_eq!(
            parser.parse_next_stmt().unwrap(),
            Some(Stmt::Assign {
                name: "msg",
                expr: Expr::Str("hello"),
            })
        );
        assert_eq!(
            parser.parse_next_stmt().unwrap(),
            Some(Stmt::Assign {
                name: "y",
                expr: Expr::Ident("x"),
            })
        );
        assert_eq!(parser.parse_next_stmt().unwrap(), None);
    }

    #[test]
    fn test_parse_empty_and_hardware_calls() {
        let mut parser = Parser::new("led_on()\ndelay_ms(500)\n").unwrap();

        assert_eq!(
            parser.parse_next_stmt().unwrap(),
            Some(Stmt::Call {
                name: "led_on",
                args: [None; MAX_CALL_ARGS],
                count: 0,
            })
        );

        let mut expected_delay_args = [None; MAX_CALL_ARGS];
        expected_delay_args[0] = Some(Expr::Int(500));

        assert_eq!(
            parser.parse_next_stmt().unwrap(),
            Some(Stmt::Call {
                name: "delay_ms",
                args: expected_delay_args,
                count: 1,
            })
        );
        assert_eq!(parser.parse_next_stmt().unwrap(), None);
    }

    #[test]
    fn test_parse_multi_arg_call_and_trailing_comma() {
        let mut parser = Parser::new("print('x =', x, 10,)\n").unwrap();

        let mut expected_args = [None; MAX_CALL_ARGS];
        expected_args[0] = Some(Expr::Str("x ="));
        expected_args[1] = Some(Expr::Ident("x"));
        expected_args[2] = Some(Expr::Int(10));

        assert_eq!(
            parser.parse_next_stmt().unwrap(),
            Some(Stmt::Call {
                name: "print",
                args: expected_args,
                count: 3,
            })
        );
    }

    #[test]
    fn test_parse_errors() {
        let mut p1 = Parser::new("123 = 456").unwrap();
        assert_eq!(p1.parse_next_stmt(), Err(Error::InvalidSyntax));

        let mut p2 = Parser::new("x = ").unwrap();
        assert_eq!(p2.parse_next_stmt(), Err(Error::UnexpectedToken));

        let mut p3 = Parser::new("led_on(1 2)").unwrap();
        assert_eq!(p3.parse_next_stmt(), Err(Error::UnexpectedToken));

        let mut p4 = Parser::new("f(1, 2, 3, 4, 5, 6, 7, 8, 9)").unwrap();
        assert_eq!(p4.parse_next_stmt(), Err(Error::TooManyArguments));
    }
}
