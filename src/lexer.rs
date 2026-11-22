// ============================================================================
// LEXER (Zero-copy tokenizer)
// ============================================================================

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Token<'a> {
    Ident(&'a str),
    Str(&'a str),
    Int(i64),
    Assign,
    Plus,
    Minus,
    Star,
    DoubleStar,
    Slash,
    DoubleSlash,
    Percent,
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
            '=' => Ok(Token::Assign),
            '+' => Ok(Token::Plus),
            '-' => Ok(Token::Minus),
            '%' => Ok(Token::Percent),
            '*' => Ok(self.lex_double('*', Token::Star, Token::DoubleStar)),
            '/' => Ok(self.lex_double('/', Token::Slash, Token::DoubleSlash)),
            '\n' => Ok(Token::Newline),
            '\'' | '"' => self.lex_string(ch),
            '0'..='9' => self.lex_int(ch),
            c if c.is_ascii_alphabetic() || c == '_' => self.lex_ident(c),
            '#' => {
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
                self.advance();
                return Ok(Token::Str(&self.src[start..end]));
            }
            self.advance();
        }
        Err(Error::UnterminatedString)
    }

    /// Lexes one- or two-character operators like `*` / `**` and `/` / `//`.
    fn lex_double(&mut self, second: char, single: Token<'a>, double: Token<'a>) -> Token<'a> {
        if self.peek() == Some(second) {
            self.advance();
            double
        } else {
            single
        }
    }

    fn lex_int(&mut self, first: char) -> Result<Token<'a>, Error> {
        let mut val = (first as u8 - b'0') as i64;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.advance();
                val = val
                    .checked_mul(10)
                    .and_then(|v| v.checked_add((ch as u8 - b'0') as i64))
                    .ok_or(Error::Overflow)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    // Helper that lexes all tokens until `Token::Eof` into a stack-allocated array.
    fn collect_tokens<'a, const N: usize>(src: &'a str) -> ([Token<'a>; N], usize) {
        let mut lexer = Lexer::new(src);
        let mut tokens = [Token::Eof; N];
        let mut count = 0;

        loop {
            let tok = lexer.next_token().expect("unexpected lexer error");
            if tok == Token::Eof {
                break;
            }
            assert!(count < N, "increase N in collect_tokens for this test");
            tokens[count] = tok;
            count += 1;
        }

        (tokens, count)
    }

    #[test]
    fn test_empty_and_whitespace_only() {
        let mut lexer = Lexer::new("   \t\r  ");
        assert_eq!(lexer.next_token(), Ok(Token::Eof));
        // Calling next_token repeatedly at EOF stays at EOF
        assert_eq!(lexer.next_token(), Ok(Token::Eof));
    }

    #[test]
    fn test_punctuation_and_newlines() {
        let (tokens, count) = collect_tokens::<8>("( ) , =\n");
        assert_eq!(
            &tokens[..count],
            &[
                Token::LParen,
                Token::RParen,
                Token::Comma,
                Token::Assign,
                Token::Newline,
            ]
        );
    }

    #[test]
    fn test_arithmetic_operators() {
        let (tokens, count) = collect_tokens::<16>("+ - * ** / // % 2**3//4");
        assert_eq!(
            &tokens[..count],
            &[
                Token::Plus,
                Token::Minus,
                Token::Star,
                Token::DoubleStar,
                Token::Slash,
                Token::DoubleSlash,
                Token::Percent,
                Token::Int(2),
                Token::DoubleStar,
                Token::Int(3),
                Token::DoubleSlash,
                Token::Int(4),
            ]
        );
    }

    #[test]
    fn test_integer_literal_overflow() {
        let mut lexer = Lexer::new("9223372036854775807");
        assert_eq!(lexer.next_token(), Ok(Token::Int(i64::MAX)));
        let mut lexer = Lexer::new("9223372036854775808");
        assert_eq!(lexer.next_token(), Err(Error::Overflow));
    }

    #[test]
    fn test_identifiers() {
        let (tokens, count) = collect_tokens::<8>("print x _private var_123");
        assert_eq!(
            &tokens[..count],
            &[
                Token::Ident("print"),
                Token::Ident("x"),
                Token::Ident("_private"),
                Token::Ident("var_123"),
            ]
        );
    }

    #[test]
    fn test_integers() {
        let (tokens, count) = collect_tokens::<8>("0 7 42 100000");
        assert_eq!(
            &tokens[..count],
            &[
                Token::Int(0),
                Token::Int(7),
                Token::Int(42),
                Token::Int(100_000),
            ]
        );
    }

    #[test]
    fn test_strings_single_and_double_quotes() {
        let (tokens, count) = collect_tokens::<8>(r#"'hello' "world" "" 'it has "quotes"'"#);
        assert_eq!(
            &tokens[..count],
            &[
                Token::Str("hello"),
                Token::Str("world"),
                Token::Str(""),
                Token::Str(r#"it has "quotes""#),
            ]
        );
    }

    #[test]
    fn test_utf8_inside_strings() {
        let (tokens, count) = collect_tokens::<4>("'Cześć 🐍'");
        assert_eq!(&tokens[..count], &[Token::Str("Cześć 🐍")]);
    }

    #[test]
    fn test_comments_are_skipped_until_newline() {
        let src = "# full line comment\nx = 10 # trailing comment\nprint(x)";
        let (tokens, count) = collect_tokens::<16>(src);
        assert_eq!(
            &tokens[..count],
            &[
                Token::Newline,
                Token::Ident("x"),
                Token::Assign,
                Token::Int(10),
                Token::Newline,
                Token::Ident("print"),
                Token::LParen,
                Token::Ident("x"),
                Token::RParen,
            ]
        );
    }

    #[test]
    fn test_comment_at_eof_without_newline() {
        let (tokens, count) = collect_tokens::<4>("42 # comment at very end");
        assert_eq!(&tokens[..count], &[Token::Int(42)]);
    }

    #[test]
    fn test_unterminated_string_error() {
        let mut lexer = Lexer::new("'hello world");
        assert_eq!(lexer.next_token(), Err(Error::UnterminatedString));
    }

    #[test]
    fn test_unexpected_char_error() {
        let mut lexer = Lexer::new("@");
        assert_eq!(lexer.next_token(), Err(Error::UnexpectedChar('@')));
        let mut lexer = Lexer::new("&");
        assert_eq!(lexer.next_token(), Err(Error::UnexpectedChar('&')));
    }
}
