// ============================================================================
// AST & PARSER (Stack-allocated, fixed capacity)
// ============================================================================

use crate::lexer::{Lexer, Token};
use crate::Error;

pub const MAX_CALL_ARGS: usize = 8;

#[derive(Debug, Clone, Copy)]
pub enum Expr<'a> {
    None,
    Str(&'a str),
    Int(i64),
    Ident(&'a str),
}

#[derive(Debug)]
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
