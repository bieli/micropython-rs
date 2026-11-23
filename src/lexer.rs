// ============================================================================
// LEXER (Zero-copy tokenizer)
// ============================================================================

use crate::ast_parser::BinOp;
use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Token<'a> {
    Ident(&'a str),
    Str(&'a str),
    /// `b'...'`: the text between the quotes, escapes not yet decoded.
    Bytes(&'a str),
    /// `f'...'`: the text between the quotes (the parser splits out the fields).
    FStr(&'a str),
    Int(i64),
    // Keywords
    If,
    Elif,
    Else,
    While,
    Break,
    Continue,
    Pass,
    Def,
    Return,
    Import,
    From,
    As,
    Global,
    For,
    Del,
    Try,
    Except,
    Finally,
    Raise,
    Class,
    With,
    Is,
    Lambda,
    Assert,
    In,
    And,
    Or,
    Not,
    True,
    False,
    None,
    // Assignment: `=` and augmented `+=`, `-=`, `<<=`, ...
    Assign,
    AugAssign(BinOp),
    // Arithmetic
    Plus,
    Minus,
    Star,
    DoubleStar,
    Slash,
    DoubleSlash,
    Percent,
    // Bitwise
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    // Comparison
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    // Punctuation
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    /// `.` in `module.name`
    Dot,
    /// `;` between statements on one line
    Semicolon,
    Newline,
    Eof,
}

/// Tab stops for indentation, as in Python.
const TAB_SIZE: u32 = 8;

#[derive(Debug, Clone, Copy)]
pub struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    /// 1-based line of the next character.
    line: u32,
    /// 1-based line where the most recently lexed token starts.
    token_line: u32,
    /// True until the first non-blank character of the current line is read.
    at_line_start: bool,
    /// Indentation (in columns) of the current line.
    line_indent: u32,
    /// Indentation of the line the most recently lexed token is on.
    token_indent: u32,
    /// Open brackets: inside `()`, `[]` and `{}` newlines don't end statements.
    depth: u16,
    /// Byte offset where the most recently lexed token starts.
    token_start: usize,
}

impl<'a> Lexer<'a> {
    pub const fn new(src: &'a str) -> Self {
        Self {
            src,
            pos: 0,
            line: 1,
            token_line: 1,
            at_line_start: true,
            line_indent: 0,
            token_indent: 0,
            depth: 0,
            token_start: 0,
        }
    }

    /// Line (1-based) where the last token returned by `next_token` starts,
    /// or where the failing token started if `next_token` returned an error.
    pub const fn token_line(&self) -> u32 {
        self.token_line
    }

    /// Indentation (in columns, tabs to multiples of 8) of the line that the
    /// last token returned by `next_token` is on.
    /// Byte offset of the most recently lexed token in the source.
    pub const fn token_start(&self) -> usize {
        self.token_start
    }

    /// The source text between two byte offsets.
    pub fn slice(&self, from: usize, to: usize) -> &'a str {
        &self.src[from..to]
    }

    pub const fn token_indent(&self) -> u32 {
        self.token_indent
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.at_line_start = true;
            self.line_indent = 0;
        }
        Some(ch)
    }

    /// Consumes `expected` if it is the next character.
    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    pub fn next_token(&mut self) -> Result<Token<'a>, Error> {
        while let Some(ch) = self.peek() {
            match ch {
                ' ' | '\t' | '\r' => {
                    if self.at_line_start {
                        self.line_indent = match ch {
                            ' ' => self.line_indent + 1,
                            '\t' => (self.line_indent / TAB_SIZE + 1) * TAB_SIZE,
                            _ => self.line_indent,
                        };
                    }
                    self.advance();
                }
                // Implicit line joining inside brackets
                '\n' if self.depth > 0 => {
                    self.advance();
                }
                '#' if self.depth > 0 => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.advance();
                    }
                }
                // Explicit line joining: a backslash at the end of a line
                '\\' if matches!(self.src.as_bytes().get(self.pos + 1), Some(b'\n' | b'\r')) => {
                    self.advance();
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.advance();
                    }
                    self.advance();
                    self.at_line_start = false;
                }
                _ => break,
            }
        }

        self.token_line = self.line;
        self.token_start = self.pos;
        self.token_indent = self.line_indent;
        let Some(ch) = self.advance() else {
            return Ok(Token::Eof);
        };
        if ch != '\n' {
            self.at_line_start = false;
        }

        let token = match ch {
            '(' | '[' | '{' => {
                self.depth = self.depth.saturating_add(1);
                match ch {
                    '(' => Token::LParen,
                    '[' => Token::LBracket,
                    _ => Token::LBrace,
                }
            }
            ')' | ']' | '}' => {
                self.depth = self.depth.saturating_sub(1);
                match ch {
                    ')' => Token::RParen,
                    ']' => Token::RBracket,
                    _ => Token::RBrace,
                }
            }
            ',' => Token::Comma,
            ':' => Token::Colon,
            ';' => Token::Semicolon,
            '.' => Token::Dot,
            '~' => Token::Tilde,
            '\n' => Token::Newline,
            '=' => self.op_or_eq(Token::Assign, Token::EqEq),
            '!' if self.eat('=') => Token::NotEq,
            '+' => self.op_or_aug(Token::Plus, BinOp::Add),
            '-' => self.op_or_aug(Token::Minus, BinOp::Sub),
            '%' => self.op_or_aug(Token::Percent, BinOp::Mod),
            '&' => self.op_or_aug(Token::Amp, BinOp::BitAnd),
            '|' => self.op_or_aug(Token::Pipe, BinOp::BitOr),
            '^' => self.op_or_aug(Token::Caret, BinOp::BitXor),
            '*' if self.eat('*') => self.op_or_aug(Token::DoubleStar, BinOp::Pow),
            '*' => self.op_or_aug(Token::Star, BinOp::Mul),
            '/' if self.eat('/') => self.op_or_aug(Token::DoubleSlash, BinOp::FloorDiv),
            '/' => self.op_or_aug(Token::Slash, BinOp::Div),
            '<' if self.eat('<') => self.op_or_aug(Token::Shl, BinOp::Shl),
            '<' => self.op_or_eq(Token::Lt, Token::Le),
            '>' if self.eat('>') => self.op_or_aug(Token::Shr, BinOp::Shr),
            '>' => self.op_or_eq(Token::Gt, Token::Ge),
            '\'' | '"' => return self.lex_string(ch),
            '0'..='9' => return self.lex_int(ch),
            'b' | 'B' | 'f' | 'F' if matches!(self.peek(), Some('\'' | '"')) => {
                let quote = self.advance().unwrap_or('\'');
                return match self.lex_string(quote)? {
                    Token::Str(raw) if matches!(ch, 'b' | 'B') => Ok(Token::Bytes(raw)),
                    Token::Str(raw) => Ok(Token::FStr(raw)),
                    other => Ok(other),
                };
            }
            c if c.is_ascii_alphabetic() || c == '_' => return Ok(self.lex_ident(c)),
            '#' => {
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        break;
                    }
                    self.advance();
                }
                return self.next_token();
            }
            other => return Err(Error::UnexpectedChar(other)),
        };
        Ok(token)
    }

    /// `op` or, if followed by `=`, the augmented assignment `op=`.
    fn op_or_aug(&mut self, op: Token<'a>, bin: BinOp) -> Token<'a> {
        if self.eat('=') {
            Token::AugAssign(bin)
        } else {
            op
        }
    }

    /// `single` or, if followed by `=`, `with_eq` (`=`/`==`, `<`/`<=`, `>`/`>=`).
    fn op_or_eq(&mut self, single: Token<'a>, with_eq: Token<'a>) -> Token<'a> {
        if self.eat('=') {
            with_eq
        } else {
            single
        }
    }

    /// A string literal, returned as written (escape sequences such as `\n`
    /// are decoded by the interpreter, so plain literals stay zero-copy).
    fn lex_string(&mut self, quote: char) -> Result<Token<'a>, Error> {
        let q = quote as u8;
        if self.src.as_bytes()[self.pos..].starts_with(&[q, q]) {
            return self.lex_long_string(q);
        }
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if ch == quote {
                let end = self.pos;
                self.advance();
                return Ok(Token::Str(&self.src[start..end]));
            }
            if ch == '\n' {
                break; // a quoted string can't span lines
            }
            self.advance();
            if ch == '\\' {
                // Skip the escaped character, so `\'` doesn't end the string
                if self.peek().is_some_and(|c| c != '\n') {
                    self.advance();
                }
            }
        }
        Err(Error::UnterminatedString)
    }

    /// A triple-quoted string (the first quote is already consumed); it may
    /// span lines.
    fn lex_long_string(&mut self, q: u8) -> Result<Token<'a>, Error> {
        self.advance();
        self.advance();
        let start = self.pos;
        while let Some(ch) = self.peek() {
            if self.src.as_bytes()[self.pos..].starts_with(&[q, q, q]) {
                let end = self.pos;
                for _ in 0..3 {
                    self.advance();
                }
                return Ok(Token::Str(&self.src[start..end]));
            }
            self.advance();
            if ch == '\\' && self.peek().is_some() {
                self.advance();
            }
        }
        Err(Error::UnterminatedString)
    }

    /// Integer literals: `42`, `1_000`, `0x1F`, `0o17`, `0b1010`.
    fn lex_int(&mut self, first: char) -> Result<Token<'a>, Error> {
        let radix = if first == '0' {
            match self.peek() {
                Some('x' | 'X') => 16,
                Some('o' | 'O') => 8,
                Some('b' | 'B') => 2,
                _ => 10,
            }
        } else {
            10
        };

        let mut val: i64 = 0;
        let mut digits = 0;
        if radix == 10 {
            val = (first as u8 - b'0') as i64;
            digits = 1;
        } else {
            self.advance(); // prefix letter
        }

        let mut last_was_underscore = false;
        while let Some(ch) = self.peek() {
            if ch == '_' {
                // `_` only between digits (or right after a base prefix)
                if last_was_underscore {
                    return Err(Error::InvalidSyntax);
                }
                last_was_underscore = true;
                self.advance();
                continue;
            }
            let Some(digit) = ch.to_digit(radix) else {
                // A letter/digit glued to the number, like `12abc` or `0b102`
                if ch.is_ascii_alphanumeric() {
                    return Err(Error::InvalidSyntax);
                }
                break;
            };
            self.advance();
            last_was_underscore = false;
            digits += 1;
            val = val
                .checked_mul(radix as i64)
                .and_then(|v| v.checked_add(digit as i64))
                .ok_or(Error::Overflow)?;
        }

        if digits == 0 || last_was_underscore {
            return Err(Error::InvalidSyntax);
        }
        Ok(Token::Int(val))
    }

    fn lex_ident(&mut self, first: char) -> Token<'a> {
        let start = self.pos - first.len_utf8();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                self.advance();
            } else {
                break;
            }
        }
        match &self.src[start..self.pos] {
            "if" => Token::If,
            "elif" => Token::Elif,
            "else" => Token::Else,
            "while" => Token::While,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "pass" => Token::Pass,
            "def" => Token::Def,
            "return" => Token::Return,
            "import" => Token::Import,
            "from" => Token::From,
            "as" => Token::As,
            "global" => Token::Global,
            "for" => Token::For,
            "del" => Token::Del,
            "try" => Token::Try,
            "except" => Token::Except,
            "finally" => Token::Finally,
            "raise" => Token::Raise,
            "class" => Token::Class,
            "with" => Token::With,
            "is" => Token::Is,
            "lambda" => Token::Lambda,
            "assert" => Token::Assert,
            "in" => Token::In,
            "and" => Token::And,
            "or" => Token::Or,
            "not" => Token::Not,
            "True" => Token::True,
            "False" => Token::False,
            "None" => Token::None,
            ident => Token::Ident(ident),
        }
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
    fn test_import_tokens() {
        let (tokens, count) = collect_tokens::<12>("import display as d\nfrom m import x\n");
        assert_eq!(
            &tokens[..count],
            &[
                Token::Import,
                Token::Ident("display"),
                Token::As,
                Token::Ident("d"),
                Token::Newline,
                Token::From,
                Token::Ident("m"),
                Token::Import,
                Token::Ident("x"),
                Token::Newline,
            ]
        );
        let (tokens, count) = collect_tokens::<4>("d.line");
        assert_eq!(
            &tokens[..count],
            &[Token::Ident("d"), Token::Dot, Token::Ident("line")]
        );
    }

    #[test]
    fn test_brackets_join_lines_and_long_strings() {
        let (tokens, count) = collect_tokens::<16>("x = [1,\n  2, # two\n  {}]\ny = 1 + \\\n 2");
        assert_eq!(
            &tokens[..count],
            &[
                Token::Ident("x"),
                Token::Assign,
                Token::LBracket,
                Token::Int(1),
                Token::Comma,
                Token::Int(2),
                Token::Comma,
                Token::LBrace,
                Token::RBrace,
                Token::RBracket,
                Token::Newline,
                Token::Ident("y"),
                Token::Assign,
                Token::Int(1),
                Token::Plus,
                Token::Int(2),
            ]
        );
        let mut lexer = Lexer::new("'''a\n'b'''\n\"\" '''x");
        assert_eq!(lexer.next_token(), Ok(Token::Str("a\n'b")));
        assert_eq!(lexer.next_token(), Ok(Token::Newline));
        assert_eq!(lexer.token_line(), 2);
        assert_eq!(lexer.next_token(), Ok(Token::Str("")));
        assert_eq!(lexer.next_token(), Err(Error::UnterminatedString));
    }

    #[test]
    fn test_bytes_literals() {
        let (tokens, count) = collect_tokens::<6>("b'ab' B\"\\x00\" b bx'' b'it\\'s'");
        assert_eq!(
            &tokens[..count],
            &[
                Token::Bytes("ab"),
                Token::Bytes("\\x00"),
                Token::Ident("b"),
                Token::Ident("bx"),
                Token::Str(""),
                Token::Bytes("it\\'s"),
            ]
        );
    }

    #[test]
    fn test_keywords() {
        let (tokens, count) = collect_tokens::<20>(
            "if elif else while break continue pass def return and or not True False None iff _if",
        );
        assert_eq!(
            &tokens[..count],
            &[
                Token::If,
                Token::Elif,
                Token::Else,
                Token::While,
                Token::Break,
                Token::Continue,
                Token::Pass,
                Token::Def,
                Token::Return,
                Token::And,
                Token::Or,
                Token::Not,
                Token::True,
                Token::False,
                Token::None,
                Token::Ident("iff"),
                Token::Ident("_if"),
            ]
        );
    }

    #[test]
    fn test_comparison_bitwise_and_punctuation() {
        let (tokens, count) = collect_tokens::<20>("== != < <= > >= & | ^ ~ << >> : = ** // ;");
        assert_eq!(
            &tokens[..count],
            &[
                Token::EqEq,
                Token::NotEq,
                Token::Lt,
                Token::Le,
                Token::Gt,
                Token::Ge,
                Token::Amp,
                Token::Pipe,
                Token::Caret,
                Token::Tilde,
                Token::Shl,
                Token::Shr,
                Token::Colon,
                Token::Assign,
                Token::DoubleStar,
                Token::DoubleSlash,
                Token::Semicolon,
            ]
        );
        let mut lexer = Lexer::new("!");
        assert_eq!(lexer.next_token(), Err(Error::UnexpectedChar('!')));
    }

    #[test]
    fn test_augmented_assignment() {
        use BinOp::*;
        let (tokens, count) = collect_tokens::<16>("+= -= *= /= //= %= **= &= |= ^= <<= >>=");
        let ops = [
            Add, Sub, Mul, Div, FloorDiv, Mod, Pow, BitAnd, BitOr, BitXor, Shl, Shr,
        ];
        assert_eq!(count, ops.len());
        for (tok, op) in tokens.iter().zip(ops) {
            assert_eq!(*tok, Token::AugAssign(op));
        }
    }

    #[test]
    fn test_radix_and_underscore_literals() {
        let (tokens, count) = collect_tokens::<8>("0x1F 0XfF 0o17 0b1010 1_000_000 0x_FF 0 007");
        assert_eq!(
            &tokens[..count],
            &[
                Token::Int(31),
                Token::Int(255),
                Token::Int(15),
                Token::Int(10),
                Token::Int(1_000_000),
                Token::Int(255),
                Token::Int(0),
                Token::Int(7),
            ]
        );
        assert_eq!(
            Lexer::new("0x7FFFFFFFFFFFFFFF").next_token(),
            Ok(Token::Int(i64::MAX))
        );
        for bad in ["0x", "0b", "0b102", "1__0", "1_", "12abc", "0xG"] {
            assert_eq!(
                Lexer::new(bad).next_token(),
                Err(Error::InvalidSyntax),
                "{}",
                bad
            );
        }
        assert_eq!(
            Lexer::new("0x8000000000000000").next_token(),
            Err(Error::Overflow)
        );
    }

    #[test]
    fn test_indentation() {
        let src = "a\n    b\n\tc\n  \t d\n\n        # comment\ne";
        let mut lexer = Lexer::new(src);
        let mut indents = [0u32; 8];
        let mut count = 0;
        loop {
            let tok = lexer.next_token().unwrap();
            match tok {
                Token::Eof => break,
                Token::Ident(_) => {
                    indents[count] = lexer.token_indent();
                    count += 1;
                }
                _ => {}
            }
        }
        // spaces count 1, a tab jumps to the next multiple of 8
        assert_eq!(&indents[..count], &[0, 4, 8, 9, 0]);
    }

    #[test]
    fn test_token_lines() {
        let mut lexer = Lexer::new("a\n\n  # comment\nb 'one line'\nc");
        let mut lines = [0u32; 8];
        let mut count = 0;
        loop {
            let tok = lexer.next_token().unwrap();
            if tok == Token::Eof {
                break;
            }
            if let Token::Ident(_) | Token::Str(_) = tok {
                lines[count] = lexer.token_line();
                count += 1;
            }
        }
        // `a` on line 1, `b` on line 4, string starts on line 4, `c` on line 5
        assert_eq!(&lines[..count], &[1, 4, 4, 5]);
    }

    #[test]
    fn test_error_line_points_at_failing_token() {
        let mut lexer = Lexer::new("x\ny\n'open");
        while lexer.next_token().is_ok() {}
        assert_eq!(lexer.token_line(), 3);
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
        let mut lexer = Lexer::new("$");
        assert_eq!(lexer.next_token(), Err(Error::UnexpectedChar('$')));
    }
}
