// ============================================================================
// LEXER (Zero-copy tokenizer)
// ============================================================================

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Token<'a> {
    Ident(&'a str),
    Str(&'a str),
    Int(i64),
    LParen,
    RParen,
    Comma,
    Newline,
    Eof,
}

pub struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub const fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    pub fn next_token(&mut self) -> Result<Token<'a>, Error> {
        // Skip horizontal whitespace (spaces/tabs)
        while let Some(ch) = self.peek() {
            if ch == ' ' || ch == '\t' || ch == '\r' {
                self.advance();
            } else {
                break;
            }
        }

        let Some(ch) = self.advance() else {
            return Ok(Token::Eof);
        };

        match ch {
            '(' => Ok(Token::LParen),
            ')' => Ok(Token::RParen),
            ',' => Ok(Token::Comma),
            '\n' => Ok(Token::Newline),
            '\'' | '"' => self.lex_string(ch),
            '0'..='9' => self.lex_int(ch),
            c if c.is_ascii_alphabetic() || c == '_' => self.lex_ident(c),
            '#' => {
                // Skip comments until newline
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        break;
                    }
                    self.advance();
                }
                self.next_token()
            }
            other => Err(Error::UnexpectedChar(other)),
        }
    }

    fn lex_string(&mut self, quote: char) -> Result<Token<'a>, Error> {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch == quote {
                let end = self.pos;
                self.advance(); // consume closing quote
                return Ok(Token::Str(&self.src[start..end]));
            }
            self.advance();
        }
        Err(Error::UnterminatedString)
    }

    fn lex_int(&mut self, first: char) -> Result<Token<'a>, Error> {
        let mut val = (first as u8 - b'0') as i64;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
                val = val * 10 + (ch as u8 - b'0') as i64;
            } else {
                break;
            }
        }
        Ok(Token::Int(val))
    }

    fn lex_ident(&mut self, first: char) -> Result<Token<'a>, Error> {
        let start = self.pos - first.len_utf8();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                self.advance();
            } else {
                break;
            }
        }
        Ok(Token::Ident(&self.src[start..self.pos]))
    }
}
