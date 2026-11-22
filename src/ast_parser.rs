// ============================================================================
// AST & PARSER (Stack-allocated, fixed capacity)
// ============================================================================

use crate::lexer::{Lexer, Token};
use crate::Error;

const MAX_CALL_ARGS: usize = 8;

#[derive(Debug, Clone, Copy)]
pub enum Expr<'a> {
    None,
    Str(&'a str),
    Int(i64),
}

#[derive(Debug)]
pub enum Stmt<'a> {
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

    pub fn parse_next_stmt(&mut self) -> Result<Option<Stmt<'a>>, Error> {
        // Consume blank lines
        while self.current == Token::Newline {
            self.bump()?;
        }

        if self.current == Token::Eof {
            return Ok(None);
        }

        let Token::Ident(func_name) = self.bump()? else {
            return Err(Error::InvalidSyntax);
        };

        if self.bump()? != Token::LParen {
            return Err(Error::UnexpectedToken);
        }

        let mut args = [Expr::None; MAX_CALL_ARGS];
        let mut arg_count = 0;

        if self.current != Token::RParen {
            loop {
                if arg_count >= MAX_CALL_ARGS {
                    return Err(Error::TooManyArgs);
                }

                args[arg_count] = match self.bump()? {
                    Token::Str(s) => Expr::Str(s),
                    Token::Int(n) => Expr::Int(n),
                    _ => return Err(Error::UnexpectedToken),
                };
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

        // Expect end of line or EOF
        match self.current {
            Token::Newline | Token::Eof => {}
            _ => return Err(Error::UnexpectedToken),
        }

        Ok(Some(Stmt::Call {
            func: func_name,
            args,
            arg_count,
        }))
    }
}
