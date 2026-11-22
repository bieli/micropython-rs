// ============================================================================
// AST & PARSER (Stack-allocated, fixed capacity)
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

#[derive(Debug, PartialEq)]
pub enum Stmt<'a> {
    Assign {
        name: &'a str,
        value: Expr<'a>,
    },
    Call {
        func: &'a str,
        args: [Expr<'a>; MAX_CALL_ARGS],
        arg_count: usize,
    },
}

pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token<'a>,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str) -> Result<Self, Error> {
        let mut lexer = Lexer::new(src);
        let current = lexer.next_token()?;
        Ok(Self { lexer, current })
    }

    fn bump(&mut self) -> Result<Token<'a>, Error> {
        let prev = self.current;
        self.current = self.lexer.next_token()?;
        Ok(prev)
    }

    fn parse_expr(&mut self) -> Result<Expr<'a>, Error> {
        match self.bump()? {
            Token::Str(s) => Ok(Expr::Str(s)),
            Token::Int(n) => Ok(Expr::Int(n)),
            Token::Ident(name) => Ok(Expr::Ident(name)),
            _ => Err(Error::UnexpectedToken),
        }
    }

    fn expect_end_of_stmt(&self) -> Result<(), Error> {
        match self.current {
            Token::Newline | Token::Eof => Ok(()),
            _ => Err(Error::UnexpectedToken),
        }
    }

    pub fn parse_next_stmt(&mut self) -> Result<Option<Stmt<'a>>, Error> {
        while self.current == Token::Newline {
            self.bump()?;
        }

        if self.current == Token::Eof {
            return Ok(None);
        }

        let Token::Ident(name) = self.bump()? else {
            return Err(Error::InvalidSyntax);
        };

        match self.bump()? {
            // Case 1: Variable assignment -> `x = 42`
            Token::Assign => {
                let value = self.parse_expr()?;
                self.expect_end_of_stmt()?;
                Ok(Some(Stmt::Assign { name, value }))
            }
            // Case 2: Function call -> `print(x, 'hi')`
            Token::LParen => {
                let mut args = [Expr::None; MAX_CALL_ARGS];
                let mut arg_count = 0;

                if self.current != Token::RParen {
                    loop {
                        if arg_count >= MAX_CALL_ARGS {
                            return Err(Error::TooManyArgs);
                        }

                        args[arg_count] = self.parse_expr()?;
                        arg_count += 1;

                        match self.current {
                            Token::Comma => {
                                self.bump()?;
                            }
                            Token::RParen => break,
                            _ => return Err(Error::UnexpectedToken),
                        }
                    }
                }

                // Consume ')'
                self.bump()?;
                self.expect_end_of_stmt()?;

                Ok(Some(Stmt::Call {
                    func: name,
                    args,
                    arg_count,
                }))
            }
            _ => Err(Error::UnexpectedToken),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_and_blank_lines() {
        let mut parser = Parser::new("\n   \n\t\n").unwrap();
        assert_eq!(parser.parse_next_stmt(), Ok(None));
        // Calling again at EOF continues returning Ok(None)
        assert_eq!(parser.parse_next_stmt(), Ok(None));
    }

    #[test]
    fn test_variable_assignments() {
        let src = r#"
x = 42
greeting = 'hello'
alias = x
"#;
        let mut parser = Parser::new(src).unwrap();

        assert_eq!(
            parser.parse_next_stmt(),
            Ok(Some(Stmt::Assign {
                name: "x",
                value: Expr::Int(42),
            }))
        );
        assert_eq!(
            parser.parse_next_stmt(),
            Ok(Some(Stmt::Assign {
                name: "greeting",
                value: Expr::Str("hello"),
            }))
        );
        assert_eq!(
            parser.parse_next_stmt(),
            Ok(Some(Stmt::Assign {
                name: "alias",
                value: Expr::Ident("x"),
            }))
        );
        assert_eq!(parser.parse_next_stmt(), Ok(None));
    }

    #[test]
    fn test_function_call_zero_args() {
        let mut parser = Parser::new("print()").unwrap();

        assert_eq!(
            parser.parse_next_stmt(),
            Ok(Some(Stmt::Call {
                func: "print",
                args: [Expr::None; MAX_CALL_ARGS],
                arg_count: 0,
            }))
        );
        assert_eq!(parser.parse_next_stmt(), Ok(None));
    }

    #[test]
    fn test_function_call_multiple_args() {
        let mut parser = Parser::new("print('count:', 10, x)").unwrap();

        let mut expected_args = [Expr::None; MAX_CALL_ARGS];
        expected_args[0] = Expr::Str("count:");
        expected_args[1] = Expr::Int(10);
        expected_args[2] = Expr::Ident("x");

        assert_eq!(
            parser.parse_next_stmt(),
            Ok(Some(Stmt::Call {
                func: "print",
                args: expected_args,
                arg_count: 3,
            }))
        );
        assert_eq!(parser.parse_next_stmt(), Ok(None));
    }

    #[test]
    fn test_syntax_errors() {
        // Statement starting with a literal instead of an identifier
        let mut p1 = Parser::new("123 = 456").unwrap();
        assert_eq!(p1.parse_next_stmt(), Err(Error::InvalidSyntax));

        // Identifier followed by neither `=` nor `(`
        let mut p2 = Parser::new("x 42").unwrap();
        assert_eq!(p2.parse_next_stmt(), Err(Error::UnexpectedToken));

        // Missing value on right-hand side of assignment
        let mut p3 = Parser::new("x = \n").unwrap();
        assert_eq!(p3.parse_next_stmt(), Err(Error::UnexpectedToken));

        // Missing comma between function arguments
        let mut p4 = Parser::new("print(1 2)").unwrap();
        assert_eq!(p4.parse_next_stmt(), Err(Error::UnexpectedToken));

        // Unclosed parenthesis
        let mut p5 = Parser::new("print('hi'\n").unwrap();
        assert_eq!(p5.parse_next_stmt(), Err(Error::UnexpectedToken));

        // Trailing tokens on the same line after a valid statement
        let mut p6 = Parser::new("x = 1 y = 2").unwrap();
        assert_eq!(p6.parse_next_stmt(), Err(Error::UnexpectedToken));
    }

    #[test]
    fn test_too_many_args_error() {
        // 9 arguments exceeds MAX_CALL_ARGS (8)
        let mut parser = Parser::new("print(1, 2, 3, 4, 5, 6, 7, 8, 9)").unwrap();
        assert_eq!(parser.parse_next_stmt(), Err(Error::TooManyArgs));
    }
}
