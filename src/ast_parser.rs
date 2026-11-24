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
// is an `Expr` range inside that buffer. Calls are expressions too:
// `f(a, b + 1)` compiles to [Ident(a), Ident(b), Int(1), Bin(Add), Call { f, 2 }].
//
// Operator precedence (lowest -> highest), matching Python:
//
//     or                      short-circuit, returns an operand (`0 or 5` == 5)
//     and                     short-circuit, returns an operand
//     not
//     == != < <= > >=         chainable: `a < b < c` == `a < b and b < c`
//     |
//     ^
//     &
//     << >>
//     + -
//     * / // %
//     unary - + ~
//     **                      right-assoc, binds tighter than unary on its left:
//                             -2**2 == -4, 2**-1 is parsed as 2**(-1)
//     atoms: int, 'str', True, False, None, name, name(args...), ( expr )
//
// Short-circuiting and chaining use forward jumps inside the RPN buffer
// (`AndJump`, `OrJump`, `CmpChain`), whose `skip` counts are patched once the
// right-hand side has been parsed.
//
// The parser returns one statement at a time. Compound statements (`if`,
// `while`, `def`, ...) only return their header; the interpreter drives the
// parser through the indented body itself (see `interpreter.rs`).

use crate::lexer::{Lexer, Token};
use crate::Error;

pub const MAX_CALL_ARGS: usize = 16;
/// Maximum number of parameters of a `def` function.
pub const MAX_PARAMS: usize = 8;
/// Maximum number of names in one `import` / `from ... import` statement.
pub const MAX_IMPORTS: usize = 8;
/// Maximum number of RPN items (operands + operators) in one statement.
pub const MAX_EXPR_ITEMS: usize = 64;
/// Maximum number of names on the left of `a, b = ...` or in `for a, b in`.
pub const MAX_TARGETS: usize = 6;
/// Maximum number of links in a comparison chain (`a < b < c` has two).
const MAX_CHAIN: usize = 16;
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
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `x in y`
    In,
    /// `x not in y`
    NotIn,
    /// `x is y`: the same object
    Is,
    /// `x is not y`
    IsNot,
}

/// One RPN instruction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item<'a> {
    Int(i64),
    Str(&'a str),
    /// A `b'...'` literal as written (escapes decoded when evaluated).
    Bytes(&'a str),
    Bool(bool),
    None,
    Ident(&'a str),
    /// Unary minus: pops one value, pushes its negation.
    Neg,
    /// Bitwise `~`: pops one value, pushes `-x - 1`.
    Invert,
    /// `not`: pops one value, pushes the inverted truth value.
    Not,
    /// Binary operator: pops rhs then lhs, pushes `lhs op rhs`.
    Bin(BinOp),
    /// Comparison: pops rhs then lhs, pushes a bool.
    Cmp(CmpOp),
    /// Link of a comparison chain (`a < b` in `a < b < c`): pops rhs and lhs.
    /// If the comparison is false, pushes `False` and skips the rest of the
    /// chain (`skip` items); otherwise pushes rhs back for the next link.
    CmpChain {
        op: CmpOp,
        skip: u8,
    },
    /// `and`: if the top value is falsy, keep it and skip `skip` items (the
    /// right operand); otherwise pop it and evaluate the right operand.
    AndJump {
        skip: u8,
    },
    /// `or`: like `AndJump`, but keeps and skips when the top value is truthy.
    OrJump {
        skip: u8,
    },
    /// Function call: pops `argc` arguments, pushes the return value. The
    /// last `kw` arguments are keyword arguments; their names are in the
    /// `kw` `KwName` items right after this one.
    Call {
        name: &'a str,
        argc: u8,
        kw: u8,
    },
    /// `.attr`: pops a module, pushes its attribute (`[Ident(d), GetAttr(WIDTH)]`).
    GetAttr(&'a str),
    /// `[i]`: pops the index and the value below it, pushes the element.
    Index,
    /// `[a:b]`: pops the bounds that are present and the value below them,
    /// pushes the slice.
    Slice {
        has_start: bool,
        has_end: bool,
    },
    /// `.attr(args)`: pops `argc` arguments and the module below them, calls
    /// the module function (`[Ident(d), args..., CallAttr { line, argc }]`).
    /// One string per item keeps `Item` at 24 bytes.
    CallAttr {
        attr: &'a str,
        argc: u8,
        kw: u8,
    },
    /// `value(args)`: calls the value below the arguments
    /// (`(lambda: 1)()`, `handlers[key]()`); keyword names follow, as for `Call`.
    CallValue {
        argc: u8,
        kw: u8,
    },
    /// The name of a keyword argument (see `Call`); does nothing by itself.
    KwName(&'a str),
    /// An f-string field: pops the value (and, with `spec`, the format spec
    /// string above it), pushes the formatted text. `conv` is `r`, `s` or 0.
    FormatValue {
        conv: u8,
        spec: bool,
    },
    /// Pops `n` strings, pushes them joined (the parts of an f-string).
    BuildStr(u8),
    /// `lambda params: expr`: the source text of the whole lambda.
    Lambda(&'a str),
    /// `[a, b]`, `(a, b)`: pops `n` values, pushes a new list / tuple.
    BuildList(u8),
    BuildTuple(u8),
    /// `{k: v, ...}`: pops `n` key/value pairs, pushes a new dict.
    BuildDict(u8),
    /// A comprehension: `[a for targets in iter if cond]` or
    /// `{a: b for targets in iter if cond}`. The header is followed by its
    /// parts, each a complete expression of the given length: `a`, `b` (dict
    /// only), `targets` `Ident` items, `iter`, `cond` (0 if there's no `if`).
    Comp {
        dict: bool,
        targets: u8,
        a: u8,
        b: u8,
        iter: u8,
        cond: u8,
    },
    /// `a if cond else b`: followed by the parts `a`, `cond` and `b`.
    IfElse {
        a: u8,
        cond: u8,
        b: u8,
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
            items: [Item::None; MAX_EXPR_ITEMS],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    /// Inserts `item` at `at`, moving the items after it. Jumps stay valid:
    /// their `skip` counts are relative, and jumps waiting to be patched are
    /// always before `at` (they belong to enclosing expressions).
    fn insert(&mut self, at: usize, item: Item<'a>) -> Result<(), Error> {
        if self.len >= MAX_EXPR_ITEMS {
            return Err(Error::OutOfMemory);
        }
        self.items.copy_within(at..self.len, at + 1);
        self.items[at] = item;
        self.len += 1;
        Ok(())
    }

    fn last(&self) -> Option<Item<'a>> {
        self.len.checked_sub(1).map(|i| self.items[i])
    }

    fn pop(&mut self) {
        self.len -= 1;
    }

    fn push(&mut self, item: Item<'a>) -> Result<(), Error> {
        if self.len >= MAX_EXPR_ITEMS {
            return Err(Error::OutOfMemory);
        }
        self.items[self.len] = item;
        self.len += 1;
        Ok(())
    }

    /// Pushes a jump with a placeholder `skip`, to be fixed by `patch_skip`.
    fn push_jump(&mut self, item: Item<'a>) -> Result<usize, Error> {
        self.push(item)?;
        Ok(self.len - 1)
    }

    /// Sets the `skip` count of the jump at `at` so that it jumps to the end
    /// of the buffer as it is now.
    fn patch_skip(&mut self, at: usize) {
        // Fits: the whole buffer is at most MAX_EXPR_ITEMS (32) items long
        let n = (self.len - at - 1) as u8;
        match &mut self.items[at] {
            Item::AndJump { skip } | Item::OrJump { skip } | Item::CmpChain { skip, .. } => {
                *skip = n
            }
            _ => unreachable!("patch_skip on a non-jump item"),
        }
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

/// `def f(a, *args)`: the last parameter (before `**kwargs`) collects the
/// extra positional arguments.
pub const STAR_ARGS: u8 = 1;
/// `def f(a, **kwargs)`: the last parameter collects extra keyword arguments.
pub const STAR_KWARGS: u8 = 2;

/// The arguments of a call, as parsed: how many, and the keyword names.
#[derive(Debug, Clone, Copy, Default)]
struct Args<'a> {
    argc: u8,
    kw: u8,
    names: [&'a str; MAX_CALL_ARGS],
}

impl<'a> Args<'a> {
    fn push_names(&self, code: &mut Code<'a>) -> Result<(), Error> {
        for &name in &self.names[..self.kw as usize] {
            code.push(Item::KwName(name))?;
        }
        Ok(())
    }
}

/// Names assigned by `a, b = ...`, `a = b = ...` or `for a, b in ...`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Targets<'a> {
    pub names: [&'a str; MAX_TARGETS],
    pub count: usize,
    /// `a, b = ...` / `for a, b in`: the value is unpacked into the names.
    pub unpack: bool,
}

impl<'a> Targets<'a> {
    pub fn names(&self) -> &[&'a str] {
        &self.names[..self.count]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stmt<'a> {
    /// `name = <expr>`
    Assign {
        name: &'a str,
        expr: Expr,
    },
    /// `a, b = <expr>` (unpacking) or `a = b = <expr>` (same value)
    AssignMulti {
        targets: Targets<'a>,
        expr: Expr,
    },
    /// `x[key] = <expr>`: `target` leaves the container and the key.
    SetItem {
        target: Expr,
        value: Expr,
    },
    /// `x[key] += <expr>`
    AugSetItem {
        target: Expr,
        op: BinOp,
        value: Expr,
    },
    /// `del a, b`
    DelNames(Targets<'a>),
    /// `del x[key]`: `target` leaves the container and the key.
    DelItem(Expr),
    /// Header `class Name:` or `class Name(Base):`
    Class {
        name: &'a str,
        base: Option<Expr>,
    },
    /// `@decorator` before a `def` or `class`
    Decorator(Expr),
    /// `obj.attr = <expr>`: `target` is the object.
    SetAttr {
        target: Expr,
        attr: &'a str,
        value: Expr,
    },
    /// `obj.attr op= <expr>`
    AugSetAttr {
        target: Expr,
        attr: &'a str,
        op: BinOp,
        value: Expr,
    },
    /// Header `with <expr>:` or `with <expr> as name:`
    With {
        expr: Expr,
        name: Option<&'a str>,
    },
    /// Header `try:`
    Try,
    /// Header `except:`, `except <classes>:` or `except <classes> as name:`
    Except {
        classes: Option<Expr>,
        name: Option<&'a str>,
    },
    /// Header `finally:`
    Finally,
    /// `raise` / `raise <expr>` (in `raise X from Y`, `Y` is parsed, not evaluated)
    Raise(Option<Expr>),
    /// `assert <cond>` / `assert <cond>, <message>`
    Assert {
        cond: Expr,
        message: Option<Expr>,
    },
    /// Header `for targets in <iter>:`
    For {
        targets: Targets<'a>,
        iter: Expr,
    },
    /// `name += <expr>` (any `BinOp`)
    AugAssign {
        name: &'a str,
        op: BinOp,
        expr: Expr,
    },
    /// Expression statement, evaluated for its side effects: `print(x)`, `led_on()`
    Expr(Expr),
    Pass,
    Break,
    Continue,
    /// `return` / `return <expr>`
    Return(Option<Expr>),
    /// Header `if <cond>:` (body follows)
    If(Expr),
    /// Header `elif <cond>:`
    Elif(Expr),
    /// Header `else:`
    Else,
    /// Header `while <cond>:`
    While(Expr),
    /// Header `def name(params...):`; `defaults` builds a tuple of the
    /// default values of the last parameters (`def f(a, b=1)`).
    Def {
        name: &'a str,
        params: [&'a str; MAX_PARAMS],
        count: usize,
        defaults: Option<Expr>,
        /// `STAR_ARGS` / `STAR_KWARGS`: the last parameters are `*args`
        /// (then) `**kwargs`.
        star: u8,
    },
    /// `import a, b as c`: `(module, bound name)` pairs.
    Import {
        names: [(&'a str, &'a str); MAX_IMPORTS],
        count: usize,
    },
    /// `global a, b`: names a function assigns as globals.
    Global {
        names: [&'a str; MAX_IMPORTS],
        count: usize,
    },
    /// `from m import a, b as c`: `(name in module, bound name)` pairs.
    FromImport {
        module: &'a str,
        names: [(&'a str, &'a str); MAX_IMPORTS],
        count: usize,
    },
}

impl Stmt<'_> {
    /// True for statements that are followed by an indented body.
    pub const fn is_compound(&self) -> bool {
        matches!(
            self,
            Stmt::If(_)
                | Stmt::Elif(_)
                | Stmt::Else
                | Stmt::While(_)
                | Stmt::For { .. }
                | Stmt::Def { .. }
                | Stmt::Try
                | Stmt::Class { .. }
                | Stmt::With { .. }
                | Stmt::Except { .. }
                | Stmt::Finally
        )
    }
}

/// The parser is a small `Copy` value: the interpreter saves a copy to jump
/// back to the start of a `while` loop or a function body.
#[derive(Debug, Clone, Copy)]
pub struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token<'a>,
    /// Line where `current` starts.
    current_line: u32,
    /// Byte offset where `current` starts.
    current_start: usize,
    /// Indentation of the line `current` is on.
    current_indent: u32,
    /// Line where the most recently parsed statement starts.
    stmt_line: u32,
    /// The last statement ended with `;`, so another may follow on its line.
    after_semicolon: bool,
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
            current_start: lexer.token_start(),
            current_indent: lexer.token_indent(),
            stmt_line: current_line,
            after_semicolon: false,
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

    /// Skips blank lines and returns the indentation of the next statement,
    /// or `None` at the end of the source.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    pub fn next_indent(&mut self) -> Result<Option<u32>, Error> {
        self.skip_newlines()?;
        Ok(match self.current {
            Token::Eof => None,
            _ => Some(self.current_indent),
        })
    }

    /// True if the parser is at the end of a line (after a `:` header this
    /// means the body is an indented block, not an inline statement).
    pub fn at_line_end(&self) -> bool {
        matches!(self.current, Token::Newline | Token::Eof)
    }

    /// True if the last statement ended with `;` (`a = 1; b = 2`), so the
    /// next statement, if any on this line, belongs to the same body.
    pub fn after_semicolon(&self) -> bool {
        self.after_semicolon
    }

    /// True if the next statement starts with `elif` / `else`.
    pub fn at_elif(&self) -> bool {
        self.current == Token::Elif
    }

    pub fn at_else(&self) -> bool {
        self.current == Token::Else
    }

    pub fn at_except(&self) -> bool {
        self.current == Token::Except
    }

    pub fn at_finally(&self) -> bool {
        self.current == Token::Finally
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn advance(&mut self) -> Result<(), Error> {
        let result = self.lexer.next_token();
        self.current_line = self.lexer.token_line();
        self.current_start = self.lexer.token_start();
        self.current_indent = self.lexer.token_indent();
        self.current = result?;
        Ok(())
    }

    /// Looks at the token after `current` without consuming anything.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn peek(&self) -> Result<Token<'a>, Error> {
        self.lexer.clone().next_token()
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn expect(&mut self, token: Token<'a>) -> Result<(), Error> {
        if self.current != token {
            return Err(Error::UnexpectedToken);
        }
        self.advance()
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn skip_newlines(&mut self) -> Result<(), Error> {
        while self.current == Token::Newline {
            self.advance()?;
        }
        Ok(())
    }

    /// A simple statement ends at a newline, the end of input, or `;`
    /// (which is consumed, so the parser is at the next statement).
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn expect_stmt_end(&mut self) -> Result<(), Error> {
        match self.current {
            Token::Newline | Token::Eof => Ok(()),
            Token::Semicolon => {
                self.after_semicolon = true;
                self.advance()
            }
            _ => Err(Error::UnexpectedToken),
        }
    }

    /// Parses the next statement. Its expressions are written into `code`
    /// (cleared first), which the caller reuses for every statement.
    ///
    /// For compound statements only the header up to and including `:` is
    /// consumed; the parser is then at the start of the body.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    pub fn parse_next_stmt(&mut self, code: &mut Code<'a>) -> Result<Option<Stmt<'a>>, Error> {
        code.clear();
        self.after_semicolon = false;
        self.skip_newlines()?;
        self.stmt_line = self.current_line;

        let stmt = match self.current {
            Token::Eof => return Ok(None),
            Token::If | Token::Elif | Token::While => {
                let keyword = self.current;
                self.advance()?;
                let cond = self.parse_expr(code)?;
                self.expect(Token::Colon)?;
                return Ok(Some(match keyword {
                    Token::If => Stmt::If(cond),
                    Token::Elif => Stmt::Elif(cond),
                    _ => Stmt::While(cond),
                }));
            }
            Token::Else => {
                self.advance()?;
                self.expect(Token::Colon)?;
                return Ok(Some(Stmt::Else));
            }
            Token::Def => return self.parse_def(code).map(Some),
            Token::Try | Token::Finally => {
                let stmt = if self.current == Token::Try {
                    Stmt::Try
                } else {
                    Stmt::Finally
                };
                self.advance()?;
                self.expect(Token::Colon)?;
                return Ok(Some(stmt));
            }
            Token::Class => {
                self.advance()?;
                let name = self.expect_ident()?;
                let mut base = None;
                if self.current == Token::LParen {
                    self.advance()?;
                    if self.current != Token::RParen {
                        base = Some(self.parse_expr(code)?);
                    }
                    self.expect(Token::RParen)?;
                }
                self.expect(Token::Colon)?;
                return Ok(Some(Stmt::Class { name, base }));
            }
            Token::At => {
                self.advance()?;
                let decorator = self.parse_expr(code)?;
                if !self.at_line_end() {
                    return Err(Error::InvalidSyntax);
                }
                return Ok(Some(Stmt::Decorator(decorator)));
            }
            Token::With => {
                self.advance()?;
                let expr = self.parse_expr(code)?;
                let name = if self.current == Token::As {
                    self.advance()?;
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                self.expect(Token::Colon)?;
                return Ok(Some(Stmt::With { expr, name }));
            }
            Token::Except => {
                self.advance()?;
                let mut classes = None;
                let mut name = None;
                if self.current != Token::Colon {
                    classes = Some(self.parse_expr(code)?);
                    if self.current == Token::As {
                        self.advance()?;
                        name = Some(self.expect_ident()?);
                    }
                }
                self.expect(Token::Colon)?;
                return Ok(Some(Stmt::Except { classes, name }));
            }
            Token::For => {
                self.advance()?;
                let targets = self.parse_targets(Token::In)?;
                self.expect(Token::In)?;
                let iter = self.parse_expr_list(code)?;
                self.expect(Token::Colon)?;
                return Ok(Some(Stmt::For { targets, iter }));
            }
            Token::Global => {
                self.advance()?;
                let mut names = [""; MAX_IMPORTS];
                let mut count = 0;
                loop {
                    if count >= MAX_IMPORTS {
                        return Err(Error::TooManyArguments);
                    }
                    names[count] = self.expect_ident()?;
                    count += 1;
                    if self.current != Token::Comma {
                        break;
                    }
                    self.advance()?;
                }
                Stmt::Global { names, count }
            }
            Token::Import => {
                self.advance()?;
                let (names, count) = self.parse_import_names()?;
                Stmt::Import { names, count }
            }
            Token::From => {
                self.advance()?;
                let module = self.expect_ident()?;
                self.expect(Token::Import)?;
                let (names, count) = self.parse_import_names()?;
                Stmt::FromImport {
                    module,
                    names,
                    count,
                }
            }
            Token::Raise => {
                self.advance()?;
                if matches!(self.current, Token::Newline | Token::Eof | Token::Semicolon) {
                    Stmt::Raise(None)
                } else {
                    let exc = self.parse_expr(code)?;
                    if self.current == Token::From {
                        self.advance()?;
                        self.parse_expr(code)?;
                    }
                    Stmt::Raise(Some(exc))
                }
            }
            Token::Assert => {
                self.advance()?;
                let cond = self.parse_expr(code)?;
                let message = if self.current == Token::Comma {
                    self.advance()?;
                    Some(self.parse_expr(code)?)
                } else {
                    None
                };
                Stmt::Assert { cond, message }
            }
            Token::Del => {
                self.advance()?;
                if let (
                    Token::Ident(_),
                    Token::Newline | Token::Eof | Token::Semicolon | Token::Comma,
                ) = (self.current, self.peek()?)
                {
                    Stmt::DelNames(self.parse_targets(Token::Newline)?)
                } else {
                    let start = code.len;
                    self.parse_test(code)?;
                    if code.last() != Some(Item::Index) {
                        return Err(Error::InvalidSyntax);
                    }
                    code.pop();
                    Stmt::DelItem(Expr {
                        start: start as u8,
                        len: (code.len - start) as u8,
                    })
                }
            }
            Token::Pass | Token::Break | Token::Continue => {
                let stmt = match self.current {
                    Token::Pass => Stmt::Pass,
                    Token::Break => Stmt::Break,
                    _ => Stmt::Continue,
                };
                self.advance()?;
                stmt
            }
            Token::Return => {
                self.advance()?;
                if self.at_line_end() {
                    Stmt::Return(None)
                } else {
                    Stmt::Return(Some(self.parse_expr_list(code)?))
                }
            }
            // Variable assignment: `x = <expr>`, `x += <expr>`
            Token::Ident(name) => match self.peek()? {
                Token::Assign => {
                    self.advance()?; // name
                    self.advance()?; // '='
                                     // `a = b = <expr>`
                    let mut targets = Targets {
                        names: [name; MAX_TARGETS],
                        count: 1,
                        unpack: false,
                    };
                    while let (Token::Ident(next), Token::Assign) = (self.current, self.peek()?) {
                        if targets.count >= MAX_TARGETS {
                            return Err(Error::TooManyArguments);
                        }
                        targets.names[targets.count] = next;
                        targets.count += 1;
                        self.advance()?;
                        self.advance()?;
                    }
                    let expr = self.parse_expr_list(code)?;
                    if targets.count == 1 {
                        Stmt::Assign { name, expr }
                    } else {
                        Stmt::AssignMulti { targets, expr }
                    }
                }
                // `a, b = <expr>`, or an expression statement like `a, b`
                Token::Comma => {
                    let saved = *self;
                    match self.parse_targets(Token::Assign) {
                        Ok(targets) if self.current == Token::Assign => {
                            self.advance()?;
                            Stmt::AssignMulti {
                                targets,
                                expr: self.parse_expr_list(code)?,
                            }
                        }
                        _ => {
                            *self = saved;
                            self.parse_expr_stmt(code)?
                        }
                    }
                }
                Token::AugAssign(op) => {
                    self.advance()?; // name
                    self.advance()?; // 'op='
                    Stmt::AugAssign {
                        name,
                        op,
                        expr: self.parse_expr(code)?,
                    }
                }
                _ => self.parse_expr_stmt(code)?,
            },
            // Expression statement: `print(x)`, `led_on()`, `delay_ms(t * 2)`,
            // or an assignment to an element: `d[k] = v`
            _ => self.parse_expr_stmt(code)?,
        };

        self.expect_stmt_end()?;
        Ok(Some(stmt))
    }

    /// An expression statement, or an assignment to an element (`x[k] = v`,
    /// `x[k] += v`): the target is parsed as an expression ending in `Index`,
    /// which is then removed, so the target leaves the container and the key.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_expr_stmt(&mut self, code: &mut Code<'a>) -> Result<Stmt<'a>, Error> {
        let start = code.len;
        self.parse_test(code)?;
        match self.current {
            Token::Assign | Token::AugAssign(_) => {
                let op = match self.current {
                    Token::AugAssign(op) => Some(op),
                    _ => None,
                };
                // `(a, b) = ...`, `[a, b] = ...`: a tuple of names unpacks
                if let (None, Some(Item::BuildTuple(n) | Item::BuildList(n))) = (op, code.last()) {
                    let names = &code.items[start..code.len - 1];
                    if names.len() == n as usize
                        && n as usize <= MAX_TARGETS
                        && names.iter().all(|i| matches!(i, Item::Ident(_)))
                    {
                        let mut targets = Targets {
                            names: [""; MAX_TARGETS],
                            count: n as usize,
                            unpack: true,
                        };
                        for (slot, item) in targets.names.iter_mut().zip(names) {
                            if let Item::Ident(name) = item {
                                *slot = name;
                            }
                        }
                        code.len = start;
                        self.advance()?;
                        let expr = self.parse_expr_list(code)?;
                        return Ok(Stmt::AssignMulti { targets, expr });
                    }
                }
                // `obj.attr = value`
                if let Some(Item::GetAttr(attr)) = code.last() {
                    code.pop();
                    let target = Expr {
                        start: start as u8,
                        len: (code.len - start) as u8,
                    };
                    self.advance()?;
                    let value = self.parse_expr_list(code)?;
                    return Ok(match op {
                        Some(op) => Stmt::AugSetAttr {
                            target,
                            attr,
                            op,
                            value,
                        },
                        None => Stmt::SetAttr {
                            target,
                            attr,
                            value,
                        },
                    });
                }
                // `123 = 456`, `f() = 1`, `a[1:2] = x`: not assignable here
                if code.last() != Some(Item::Index) {
                    return Err(Error::InvalidSyntax);
                }
                code.pop();
                let target = Expr {
                    start: start as u8,
                    len: (code.len - start) as u8,
                };
                self.advance()?;
                let value = self.parse_expr_list(code)?;
                Ok(match op {
                    Some(op) => Stmt::AugSetItem { target, op, value },
                    None => Stmt::SetItem { target, value },
                })
            }
            Token::Comma => {
                let n = self.parse_more_elements(code, Token::Newline)?;
                code.push(Item::BuildTuple(n))?;
                Ok(Stmt::Expr(Expr {
                    start: start as u8,
                    len: (code.len - start) as u8,
                }))
            }
            _ => Ok(Stmt::Expr(Expr {
                start: start as u8,
                len: (code.len - start) as u8,
            })),
        }
    }

    /// targets := IDENT (',' IDENT)* [','] | '(' targets ')', up to `end`.
    fn parse_targets(&mut self, end: Token<'a>) -> Result<Targets<'a>, Error> {
        let parens = self.current == Token::LParen;
        if parens {
            self.advance()?;
        }
        let mut t = Targets {
            names: [""; MAX_TARGETS],
            count: 0,
            unpack: false,
        };
        loop {
            if t.count >= MAX_TARGETS {
                return Err(Error::TooManyArguments);
            }
            t.names[t.count] = self.expect_ident()?;
            t.count += 1;
            if self.current != Token::Comma {
                break;
            }
            t.unpack = true;
            self.advance()?;
            if self.current == end || self.current == Token::RParen {
                break;
            }
        }
        if parens {
            self.expect(Token::RParen)?;
            t.unpack = true;
        }
        Ok(t)
    }

    /// import_names := IDENT ['as' IDENT] (',' IDENT ['as' IDENT])*
    #[allow(clippy::type_complexity)]
    fn parse_import_names(&mut self) -> Result<([(&'a str, &'a str); MAX_IMPORTS], usize), Error> {
        let mut names = [("", ""); MAX_IMPORTS];
        let mut count = 0;
        loop {
            if count >= MAX_IMPORTS {
                return Err(Error::TooManyArguments);
            }
            let name = self.expect_ident()?;
            let bound = if self.current == Token::As {
                self.advance()?;
                self.expect_ident()?
            } else {
                name
            };
            names[count] = (name, bound);
            count += 1;
            if self.current != Token::Comma {
                return Ok((names, count));
            }
            self.advance()?;
        }
    }

    fn expect_ident(&mut self) -> Result<&'a str, Error> {
        let Token::Ident(name) = self.current else {
            return Err(Error::InvalidSyntax);
        };
        self.advance()?;
        Ok(name)
    }

    /// def := 'def' IDENT '(' [param (',' param)* [',']] ')' ':'
    /// with param := IDENT ['=' expr]
    #[inline(never)]
    fn parse_def(&mut self, code: &mut Code<'a>) -> Result<Stmt<'a>, Error> {
        let defaults_start = code.len;
        let mut ndefaults = 0u8;
        self.advance()?; // 'def'
        let Token::Ident(name) = self.current else {
            return Err(Error::InvalidSyntax);
        };
        self.advance()?;
        self.expect(Token::LParen)?;

        let mut params = [""; MAX_PARAMS];
        let mut count = 0;
        let mut star = 0u8;
        while self.current != Token::RParen {
            // `*args` and then `**kwargs` come last
            let marker = match self.current {
                Token::Star if star == 0 => STAR_ARGS,
                Token::DoubleStar if star & STAR_KWARGS == 0 => STAR_KWARGS,
                _ if star & STAR_KWARGS != 0 => return Err(Error::InvalidSyntax),
                _ => 0,
            };
            if marker != 0 {
                self.advance()?;
            } else if star != 0 {
                return Err(Error::InvalidSyntax); // keyword-only parameters aren't supported
            }
            star |= marker;
            let Token::Ident(param) = self.current else {
                return Err(Error::InvalidSyntax);
            };
            if count >= MAX_PARAMS {
                return Err(Error::TooManyArguments);
            }
            // Duplicate parameter names are a syntax error in Python
            if params[..count].contains(&param) {
                return Err(Error::InvalidSyntax);
            }
            params[count] = param;
            count += 1;
            self.advance()?;
            if marker == 0 && self.current == Token::Assign {
                self.advance()?;
                self.parse_test(code)?;
                ndefaults += 1;
            } else if ndefaults > 0 {
                // A parameter without a default after one with a default
                return Err(Error::InvalidSyntax);
            }

            match self.current {
                Token::Comma => self.advance()?,
                Token::RParen => break,
                _ => return Err(Error::UnexpectedToken),
            }
        }

        self.advance()?; // ')'
        self.expect(Token::Colon)?;
        let defaults = if ndefaults > 0 {
            code.push(Item::BuildTuple(ndefaults))?;
            Some(Expr {
                start: defaults_start as u8,
                len: (code.len - defaults_start) as u8,
            })
        } else {
            None
        };
        Ok(Stmt::Def {
            name,
            params,
            count,
            defaults,
            star,
        })
    }

    /// Parses one full expression and returns its range in `code`.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_expr(&mut self, code: &mut Code<'a>) -> Result<Expr, Error> {
        let start = code.len;
        self.parse_test(code)?;
        Ok(Expr {
            start: start as u8,
            len: (code.len - start) as u8,
        })
    }

    /// Like `parse_expr`, but `a, b` (without parentheses) is a tuple, as on
    /// the right of `=`, after `return` and after `for ... in`.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_expr_list(&mut self, code: &mut Code<'a>) -> Result<Expr, Error> {
        let start = code.len;
        self.parse_test(code)?;
        if self.current == Token::Comma {
            let n = self.parse_more_elements(code, Token::Colon)?;
            code.push(Item::BuildTuple(n))?;
        }
        Ok(Expr {
            start: start as u8,
            len: (code.len - start) as u8,
        })
    }

    /// After a first element, at a `,`: parses `, b, c [,]` up to `end` (or
    /// the end of the line); returns the number of elements including the first.
    fn parse_more_elements(&mut self, code: &mut Code<'a>, end: Token<'a>) -> Result<u8, Error> {
        let mut n = 1u8;
        while self.current == Token::Comma {
            self.advance()?;
            if self.current == end
                || matches!(
                    self.current,
                    Token::Newline
                        | Token::Eof
                        | Token::Semicolon
                        | Token::RParen
                        | Token::RBracket
                        | Token::Assign
                )
            {
                break;
            }
            self.nested(|p| p.parse_test(code))?;
            n = n.checked_add(1).ok_or(Error::TooManyArguments)?;
        }
        Ok(n)
    }

    /// test := or ['if' or 'else' test] | lambda
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_test(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        if self.current == Token::Lambda {
            return self.parse_lambda(code);
        }
        let start = code.len;
        self.parse_or(code)?;
        if self.current != Token::If {
            return Ok(());
        }
        let a = code.len - start;
        self.advance()?;
        self.nested(|p| p.parse_or(code))?;
        let cond = code.len - start - a;
        self.expect(Token::Else)?;
        self.nested(|p| p.parse_test(code))?;
        let b = code.len - start - a - cond;
        code.insert(
            start,
            Item::IfElse {
                a: a as u8,
                cond: cond as u8,
                b: b as u8,
            },
        )
    }

    /// The rest of a comprehension, at `for`: the element part(s) are
    /// already in `code` from `start` (`a` and, for a dict, `b` items).
    #[inline(never)]
    fn parse_comprehension(
        &mut self,
        code: &mut Code<'a>,
        start: usize,
        dict: bool,
        a: usize,
    ) -> Result<(), Error> {
        let b = code.len - start - a;
        self.advance()?; // 'for'
        let targets = self.parse_targets(Token::In)?;
        for &name in targets.names() {
            code.push(Item::Ident(name))?;
        }
        self.expect(Token::In)?;
        let before_iter = code.len;
        self.nested(|p| p.parse_or(code))?;
        let iter = code.len - before_iter;
        let mut cond = 0;
        if self.current == Token::If {
            self.advance()?;
            let before = code.len;
            self.nested(|p| p.parse_or(code))?;
            cond = code.len - before;
        }
        if self.current == Token::For {
            return Err(Error::InvalidSyntax); // one `for` per comprehension
        }
        code.insert(
            start,
            Item::Comp {
                dict,
                targets: targets.count as u8,
                a: a as u8,
                b: b as u8,
                iter: iter as u8,
                cond: cond as u8,
            },
        )
    }

    /// `[...]`, `(...)` and `{...}` displays, at the opening bracket.
    #[inline(never)]
    fn parse_display(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        let open = self.current;
        self.advance()?;
        let (close, build): (Token<'a>, fn(u8) -> Item<'a>) = match open {
            Token::LParen => (Token::RParen, Item::BuildTuple),
            Token::LBracket => (Token::RBracket, Item::BuildList),
            _ => (Token::RBrace, Item::BuildDict),
        };
        if self.current == close {
            self.advance()?;
            return code.push(build(0));
        }
        let start = code.len;
        self.parse_test(code)?;
        let key_len = code.len - start;
        let dict = open == Token::LBrace;
        if dict {
            // Sets (`{a, b}`) aren't supported
            self.expect(Token::Colon)?;
            self.nested(|p| p.parse_test(code))?;
        }
        if self.current == Token::For {
            let a = if dict { key_len } else { code.len - start };
            self.parse_comprehension(code, start, dict, a)?;
            // `(x for x in y)` is evaluated as a list (no generators)
            return self.expect(close);
        }
        let mut n = 1u8;
        if dict {
            while self.current == Token::Comma {
                self.advance()?;
                if self.current == close {
                    break;
                }
                self.nested(|p| p.parse_test(code))?;
                self.expect(Token::Colon)?;
                self.nested(|p| p.parse_test(code))?;
                n = n.checked_add(1).ok_or(Error::TooManyArguments)?;
            }
        } else if self.current == Token::Comma {
            n = self.parse_more_elements(code, close)?;
        } else if open == Token::LParen {
            // `(x)` is just `x`
            return self.expect(close);
        }
        self.expect(close)?;
        code.push(build(n))
    }

    /// or := and ('or' and)*
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_or(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_and(code)?;
        while self.current == Token::Or {
            self.advance()?;
            let jump = code.push_jump(Item::OrJump { skip: 0 })?;
            self.parse_and(code)?;
            code.patch_skip(jump);
        }
        Ok(())
    }

    /// and := not ('and' not)*
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_and(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_not(code)?;
        while self.current == Token::And {
            self.advance()?;
            let jump = code.push_jump(Item::AndJump { skip: 0 })?;
            self.parse_not(code)?;
            code.patch_skip(jump);
        }
        Ok(())
    }

    /// not := 'not' not | comparison
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_not(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        if self.current == Token::Not {
            self.advance()?;
            self.nested(|p| p.parse_not(code))?;
            return code.push(Item::Not);
        }
        self.parse_comparison(code)
    }

    /// comparison := bitor (cmp_op bitor)*
    ///
    /// `a < b < c` compiles to [a, b, CmpChain(Lt, skip=2), c, Cmp(Lt)], so
    /// `b` is evaluated only once and `c` is skipped if `a < b` is false.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_comparison(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_bitor(code)?;
        let Some(mut op) = self.take_cmp_op()? else {
            return Ok(());
        };

        let mut chain_jumps = [0u8; MAX_CHAIN];
        let mut links = 0;
        loop {
            self.parse_bitor(code)?;
            match self.take_cmp_op()? {
                Some(next) => {
                    if links >= MAX_CHAIN {
                        return Err(Error::OutOfMemory);
                    }
                    chain_jumps[links] = code.push_jump(Item::CmpChain { op, skip: 0 })? as u8;
                    links += 1;
                    op = next;
                }
                None => {
                    code.push(Item::Cmp(op))?;
                    break;
                }
            }
        }
        for &jump in &chain_jumps[..links] {
            code.patch_skip(jump as usize);
        }
        Ok(())
    }

    /// Consumes a comparison operator (`not in` is two tokens), if any.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn take_cmp_op(&mut self) -> Result<Option<CmpOp>, Error> {
        let op = match self.current {
            Token::Not if self.peek()? == Token::In => {
                self.advance()?;
                CmpOp::NotIn
            }
            Token::In => CmpOp::In,
            Token::Is if self.peek()? == Token::Not => {
                self.advance()?;
                CmpOp::IsNot
            }
            Token::Is => CmpOp::Is,
            other => match cmp_op(other) {
                Some(op) => op,
                None => return Ok(None),
            },
        };
        self.advance()?;
        Ok(Some(op))
    }

    /// bitor := bitxor ('|' bitxor)*
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_bitor(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_left_assoc(code, Self::parse_bitxor, |t| match t {
            Token::Pipe => Some(BinOp::BitOr),
            _ => None,
        })
    }

    /// bitxor := bitand ('^' bitand)*
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_bitxor(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_left_assoc(code, Self::parse_bitand, |t| match t {
            Token::Caret => Some(BinOp::BitXor),
            _ => None,
        })
    }

    /// bitand := shift ('&' shift)*
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_bitand(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_left_assoc(code, Self::parse_shift, |t| match t {
            Token::Amp => Some(BinOp::BitAnd),
            _ => None,
        })
    }

    /// shift := additive (('<<' | '>>') additive)*
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_shift(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_left_assoc(code, Self::parse_additive, |t| match t {
            Token::Shl => Some(BinOp::Shl),
            Token::Shr => Some(BinOp::Shr),
            _ => None,
        })
    }

    /// additive := term (('+' | '-') term)*
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_additive(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_left_assoc(code, Self::parse_term, |t| match t {
            Token::Plus => Some(BinOp::Add),
            Token::Minus => Some(BinOp::Sub),
            _ => None,
        })
    }

    /// term := unary (('*' | '/' | '//' | '%') unary)*
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_term(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_left_assoc(code, Self::parse_unary, |t| match t {
            Token::Star => Some(BinOp::Mul),
            Token::Slash => Some(BinOp::Div),
            Token::DoubleSlash => Some(BinOp::FloorDiv),
            Token::Percent => Some(BinOp::Mod),
            _ => None,
        })
    }

    /// Shared loop for left-associative binary operator levels:
    /// `operand (op operand)*`, where `op_for` maps a token to its operator.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_left_assoc(
        &mut self,
        code: &mut Code<'a>,
        operand: fn(&mut Self, &mut Code<'a>) -> Result<(), Error>,
        op_for: fn(Token<'a>) -> Option<BinOp>,
    ) -> Result<(), Error> {
        operand(self, code)?;
        while let Some(op) = op_for(self.current) {
            self.advance()?;
            operand(self, code)?;
            code.push(Item::Bin(op))?;
        }
        Ok(())
    }

    /// unary := ('-' | '+' | '~') unary | power
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_unary(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        let item = match self.current {
            Token::Minus => Some(Item::Neg),
            Token::Tilde => Some(Item::Invert),
            Token::Plus => None,
            _ => return self.parse_power(code),
        };
        self.advance()?;
        self.nested(|p| p.parse_unary(code))?;
        match item {
            Some(item) => code.push(item),
            None => Ok(()), // unary `+` is a no-op
        }
    }

    /// power := atom ('**' unary)?
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_power(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_postfix(code)?;
        if self.current == Token::DoubleStar {
            self.advance()?;
            self.nested(|p| p.parse_unary(code))?;
            code.push(Item::Bin(BinOp::Pow))?;
        }
        Ok(())
    }

    /// postfix := atom ('.' IDENT ['(' args ')'] | '[' index_or_slice ']')*
    ///
    /// `s.strip().upper()`, `d.line(1, 2)`, `s[i]`, `s[a:b]`: the value is
    /// already on the stack when a trailer is applied.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_postfix(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        self.parse_atom(code)?;
        loop {
            match self.current {
                Token::Dot => {
                    self.advance()?;
                    let attr = self.expect_ident()?;
                    if self.current == Token::LParen {
                        let mut args = Args::default();
                        self.nested(|p| {
                            args = p.parse_args(code)?;
                            Ok(())
                        })?;
                        code.push(Item::CallAttr {
                            attr,
                            argc: args.argc,
                            kw: args.kw,
                        })?;
                        args.push_names(code)?;
                    } else {
                        code.push(Item::GetAttr(attr))?;
                    }
                }
                Token::LParen => {
                    let mut args = Args::default();
                    self.nested(|p| {
                        args = p.parse_args(code)?;
                        Ok(())
                    })?;
                    code.push(Item::CallValue {
                        argc: args.argc,
                        kw: args.kw,
                    })?;
                    args.push_names(code)?;
                }
                Token::LBracket => {
                    self.advance()?;
                    let has_start = self.current != Token::Colon;
                    if has_start {
                        self.nested(|p| p.parse_test(code))?;
                    }
                    if self.current == Token::Colon {
                        self.advance()?;
                        let has_end = self.current != Token::RBracket;
                        if has_end {
                            self.nested(|p| p.parse_test(code))?;
                        }
                        code.push(Item::Slice { has_start, has_end })?;
                    } else {
                        code.push(Item::Index)?;
                    }
                    self.expect(Token::RBracket)?;
                }
                _ => return Ok(()),
            }
        }
    }

    /// atom := INT | STR | True | False | None | IDENT | call | '(' expr ')'
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_atom(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        let item = match self.current {
            Token::Int(n) => Item::Int(n),
            Token::Str(_) | Token::Bytes(_) | Token::FStr(_) => return self.parse_strings(code),
            Token::True => Item::Bool(true),
            Token::False => Item::Bool(false),
            Token::None => Item::None,
            Token::Ident(name) => {
                self.advance()?;
                if self.current == Token::LParen {
                    return self.nested(|p| p.parse_call(name, code));
                }
                return code.push(Item::Ident(name));
            }
            Token::LParen | Token::LBracket | Token::LBrace => {
                return self.nested(|p| p.parse_display(code));
            }
            _ => return Err(Error::UnexpectedToken),
        };
        self.advance()?;
        code.push(item)
    }

    /// String literals, and adjacent ones joined (`'abc' "def"`, also over
    /// lines inside brackets): each is emitted as usual (zero-copy, with its
    /// own escapes), followed by an addition.
    fn parse_strings(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        let mut first = true;
        loop {
            match self.current {
                Token::Str(s) => code.push(Item::Str(s))?,
                Token::Bytes(s) => code.push(Item::Bytes(s))?,
                Token::FStr(s) => self.parse_fstring(s, code)?,
                _ => return Ok(()),
            }
            self.advance()?;
            if !first {
                code.push(Item::Bin(BinOp::Add))?;
            }
            first = false;
        }
    }

    /// call := IDENT '(' [expr (',' expr)* [',']] ')'
    ///
    /// Arguments are emitted first, then `Call { name, argc, kw }`.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_call(&mut self, name: &'a str, code: &mut Code<'a>) -> Result<(), Error> {
        let args = self.parse_args(code)?;
        code.push(Item::Call {
            name,
            argc: args.argc,
            kw: args.kw,
        })?;
        args.push_names(code)
    }

    /// '(' [arg (',' arg)* [',']] ')' with arg := expr | IDENT '=' expr:
    /// emits the argument values; keyword arguments come last.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_args(&mut self, code: &mut Code<'a>) -> Result<Args<'a>, Error> {
        // Consume '('
        self.advance()?;

        let mut args = Args::default();
        let mut argc = 0;
        // After a keyword argument, only keyword arguments and `**` may follow
        let mut keywords = false;
        while self.current != Token::RParen {
            if argc >= MAX_CALL_ARGS {
                return Err(Error::TooManyArguments);
            }
            // `f(*items)` / `f(**options)`: unpacked when called (the name
            // `*` or `**` marks them among the keyword names)
            if matches!(self.current, Token::Star | Token::DoubleStar) {
                let name = if self.current == Token::Star {
                    "*"
                } else {
                    "**"
                };
                if name == "*" && keywords {
                    return Err(Error::InvalidSyntax);
                }
                self.advance()?;
                self.parse_test(code)?;
                args.names[args.kw as usize] = name;
                args.kw += 1;
                argc += 1;
                match self.current {
                    Token::Comma => self.advance()?,
                    Token::RParen => break,
                    _ => return Err(Error::UnexpectedToken),
                }
                continue;
            }
            if let (Token::Ident(name), Token::Assign) = (self.current, self.peek()?) {
                // `name=value`
                if args.names[..args.kw as usize].contains(&name) && !matches!(name, "*" | "**") {
                    return Err(Error::InvalidSyntax); // repeated keyword
                }
                self.advance()?;
                self.advance()?;
                self.parse_test(code)?;
                args.names[args.kw as usize] = name;
                args.kw += 1;
                argc += 1;
                keywords = true;
                match self.current {
                    Token::Comma => self.advance()?,
                    Token::RParen => break,
                    _ => return Err(Error::UnexpectedToken),
                }
                continue;
            }
            if keywords {
                // A positional argument after a keyword argument
                return Err(Error::InvalidSyntax);
            }
            if args.kw > 0 {
                // A positional argument after `*items`: unnamed, in order
                self.parse_test(code)?;
                args.names[args.kw as usize] = "";
                args.kw += 1;
                argc += 1;
                match self.current {
                    Token::Comma => self.advance()?,
                    Token::RParen => break,
                    _ => return Err(Error::UnexpectedToken),
                }
                continue;
            }
            let start = code.len;
            self.parse_test(code)?;
            argc += 1;
            // `sum(x for x in y)`: a generator as the only argument
            if argc == 1 && self.current == Token::For {
                self.parse_comprehension(code, start, false, code.len - start)?;
                break;
            }

            match self.current {
                // A trailing comma before ')' is allowed
                Token::Comma => self.advance()?,
                Token::RParen => break,
                _ => return Err(Error::UnexpectedToken),
            }
        }

        // Consume ')'
        self.advance()?;
        args.argc = argc as u8;
        Ok(args)
    }

    /// lambda := 'lambda' [IDENT (',' IDENT)*] ':' test
    ///
    /// The body is parsed (to check it and to find its end) into a scratch
    /// buffer; the interpreter parses the lambda's text again when it's called.
    #[inline(never)]
    fn parse_lambda(&mut self, code: &mut Code<'a>) -> Result<(), Error> {
        let start = self.lexer_token_start();
        self.advance()?; // 'lambda'
        while self.current != Token::Colon {
            if matches!(self.current, Token::Star | Token::DoubleStar) {
                self.advance()?;
            }
            self.expect_ident()?;
            match self.current {
                Token::Comma => self.advance()?,
                Token::Colon => break,
                _ => return Err(Error::UnexpectedToken),
            }
        }
        self.advance()?; // ':'
        let mut scratch = Code::new();
        self.nested(|p| p.parse_test(&mut scratch))?;
        let end = self.lexer_token_start();
        code.push(Item::Lambda(self.lexer.slice(start, end).trim_end()))
    }

    /// Start of the token the parser is looking at (`current`).
    fn lexer_token_start(&self) -> usize {
        self.current_start
    }

    /// Parses `lambda params: body` from the text of a lambda: returns the
    /// parameter names and the body as an expression in `code`.
    #[allow(clippy::type_complexity)]
    pub fn parse_lambda_parts(
        &mut self,
        code: &mut Code<'a>,
    ) -> Result<([&'a str; MAX_PARAMS], usize, u8, Expr), Error> {
        self.expect(Token::Lambda)?;
        let mut params = [""; MAX_PARAMS];
        let mut count = 0;
        let mut star = 0;
        while self.current != Token::Colon {
            if count >= MAX_PARAMS {
                return Err(Error::TooManyArguments);
            }
            match self.current {
                Token::Star => star |= STAR_ARGS,
                Token::DoubleStar => star |= STAR_KWARGS,
                _ => {}
            }
            if matches!(self.current, Token::Star | Token::DoubleStar) {
                self.advance()?;
            }
            params[count] = self.expect_ident()?;
            count += 1;
            if self.current == Token::Comma {
                self.advance()?;
            }
        }
        self.advance()?;
        let body = self.parse_expr(code)?;
        Ok((params, count, star, body))
    }

    /// The parts of an f-string: literal text (as zero-copy `Str` items, so
    /// escapes work as usual) and `{expr!conv:spec}` fields, whose
    /// expressions are parsed from the source in place. Then `BuildStr`.
    #[inline(never)]
    fn parse_fstring(&mut self, raw: &'a str, code: &mut Code<'a>) -> Result<(), Error> {
        let bytes = raw.as_bytes();
        let (mut i, mut lit, mut parts) = (0, 0, 0u8);
        let part = |code: &mut Code<'a>, item: Item<'a>, parts: &mut u8| -> Result<(), Error> {
            *parts = parts.checked_add(1).ok_or(Error::TooManyArguments)?;
            code.push(item)
        };
        while i < bytes.len() {
            match (bytes[i], bytes.get(i + 1)) {
                // `{{` and `}}`: the text so far plus one brace
                (b'{', Some(b'{')) | (b'}', Some(b'}')) => {
                    part(code, Item::Str(&raw[lit..i + 1]), &mut parts)?;
                    i += 2;
                    lit = i;
                }
                (b'}', _) => return Err(Error::InvalidSyntax), // single '}'
                (b'{', _) => {
                    if i > lit {
                        part(code, Item::Str(&raw[lit..i]), &mut parts)?;
                    }
                    // The expression ends at a `!` or `:` outside brackets, or `}`
                    let start = i + 1;
                    let (mut depth, mut j, mut quote) = (0i32, start, 0u8);
                    while j < bytes.len() {
                        let c = bytes[j];
                        match c {
                            _ if quote != 0 => {
                                if c == quote {
                                    quote = 0;
                                }
                            }
                            b'\'' | b'"' => quote = c,
                            b'(' | b'[' | b'{' => depth += 1,
                            b')' | b']' => depth -= 1,
                            b'}' if depth > 0 => depth -= 1,
                            b'}' => break,
                            b'!' if depth == 0 && bytes.get(j + 1) != Some(&b'=') => break,
                            b':' if depth == 0 => break,
                            _ => {}
                        }
                        j += 1;
                    }
                    let expr = &raw[start..j];
                    let mut sub = Parser::new(expr)?;
                    self.nested(|_| sub.parse_test(code))?;
                    if sub.current != Token::Eof || expr.trim().is_empty() {
                        return Err(Error::InvalidSyntax);
                    }
                    let mut conv = 0u8;
                    if bytes.get(j) == Some(&b'!') {
                        conv = *bytes.get(j + 1).ok_or(Error::InvalidSyntax)?;
                        if !matches!(conv, b'r' | b's' | b'a') {
                            return Err(Error::InvalidSyntax);
                        }
                        j += 2;
                    }
                    let mut spec = false;
                    if bytes.get(j) == Some(&b':') {
                        // The spec may contain fields itself: `{x:>{width}}`
                        let mut close = j + 1;
                        let mut nesting = 0;
                        while close < bytes.len() {
                            match bytes[close] {
                                b'{' => nesting += 1,
                                b'}' if nesting == 0 => break,
                                b'}' => nesting -= 1,
                                _ => {}
                            }
                            close += 1;
                        }
                        let text = &raw[j + 1..close.min(raw.len())];
                        if text.contains('{') {
                            self.nested(|p| p.parse_fstring(text, code))?;
                        } else {
                            code.push(Item::Str(text))?;
                        }
                        spec = true;
                        j = close;
                    }
                    if bytes.get(j) != Some(&b'}') {
                        return Err(Error::InvalidSyntax); // f-string: expecting '}'
                    }
                    part(code, Item::FormatValue { conv, spec }, &mut parts)?;
                    i = j + 1;
                    lit = i;
                }
                _ => i += 1,
            }
        }
        if lit < bytes.len() || parts == 0 {
            part(code, Item::Str(&raw[lit..]), &mut parts)?;
        }
        code.push(Item::BuildStr(parts))
    }

    /// Runs `f` one nesting level deeper, failing instead of overflowing the
    /// native stack on inputs like `((((((...))))))` or `- - - - - 1`.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
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

fn cmp_op(token: Token<'_>) -> Option<CmpOp> {
    Some(match token {
        Token::EqEq => CmpOp::Eq,
        Token::NotEq => CmpOp::Ne,
        Token::Lt => CmpOp::Lt,
        Token::Le => CmpOp::Le,
        Token::Gt => CmpOp::Gt,
        Token::Ge => CmpOp::Ge,
        _ => return None,
    })
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
                argc: 0,
                kw: 0
            }
        );

        let items = expr_stmt(&mut parser, &mut code);
        assert_eq!(
            &items[..code.len],
            &[
                Int(500),
                Call {
                    name: "delay_ms",
                    argc: 1,
                    kw: 0
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
                    argc: 3,
                    kw: 0
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
                    argc: 2,
                    kw: 0
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
                    kw: 0,
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
                Call {
                    name: "g",
                    argc: 1,
                    kw: 0,
                },
                Int(2),
                Call {
                    name: "h",
                    argc: 0,
                    kw: 0,
                },
                Bin(Add),
                Call {
                    name: "f",
                    argc: 2,
                    kw: 0,
                },
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
        // Inside brackets the statement continues, up to the end of the input
        assert_eq!(parser.line(), 3);

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
    fn test_parse_logic_and_comparison_jumps() {
        use CmpOp::{Lt, Ne};
        use Item::{AndJump, Cmp, CmpChain, Ident, Int, Not, OrJump};

        // `a and b or c`: the `and` jump skips `b`, the `or` jump skips `c`
        assert_rpn(
            "x = a and b or c",
            &[
                Ident("a"),
                AndJump { skip: 1 },
                Ident("b"),
                OrJump { skip: 1 },
                Ident("c"),
            ],
        );
        // `a or b and c`: `and` binds tighter, so the `or` jump skips `b and c`
        assert_rpn(
            "x = a or b and c",
            &[
                Ident("a"),
                OrJump { skip: 3 },
                Ident("b"),
                AndJump { skip: 1 },
                Ident("c"),
            ],
        );
        // Chained comparison: a false first link skips to the end of the chain
        assert_rpn(
            "x = 1 < y < 3 != 4",
            &[
                Int(1),
                Ident("y"),
                CmpChain { op: Lt, skip: 4 },
                Int(3),
                CmpChain { op: Lt, skip: 2 },
                Int(4),
                Cmp(Ne),
            ],
        );
        assert_rpn("x = not a < 1", &[Ident("a"), Int(1), Cmp(Lt), Not]);
    }

    #[test]
    fn test_parse_bitwise_precedence() {
        use BinOp::*;
        use Item::{Bin, Ident, Int, Invert};
        // | < ^ < & < shifts < + -
        assert_rpn(
            "x = a | b ^ c & d << 1 + 2",
            &[
                Ident("a"),
                Ident("b"),
                Ident("c"),
                Ident("d"),
                Int(1),
                Int(2),
                Bin(Add),
                Bin(Shl),
                Bin(BitAnd),
                Bin(BitXor),
                Bin(BitOr),
            ],
        );
        assert_rpn(
            "x = ~a & 0xFF",
            &[Ident("a"), Invert, Int(255), Bin(BitAnd)],
        );
    }

    #[test]
    fn test_parse_compound_headers_and_simple_statements() {
        let mut code = Code::new();
        let mut parser = Parser::new(
            "if a:\nelif b:\nelse:\nwhile 1:\ndef f(x, y,):\npass\nbreak\ncontinue\nreturn\nreturn 1\nn += 2\n",
        )
        .unwrap();

        let mut next = || parser.parse_next_stmt(&mut code).unwrap().unwrap();
        assert!(matches!(next(), Stmt::If(_)));
        assert!(matches!(next(), Stmt::Elif(_)));
        assert_eq!(next(), Stmt::Else);
        assert!(matches!(next(), Stmt::While(_)));
        let Stmt::Def {
            name,
            params,
            count,
            ..
        } = next()
        else {
            panic!("expected def");
        };
        assert_eq!((name, &params[..count]), ("f", &["x", "y"][..]));
        assert_eq!(next(), Stmt::Pass);
        assert_eq!(next(), Stmt::Break);
        assert_eq!(next(), Stmt::Continue);
        assert_eq!(next(), Stmt::Return(None));
        assert!(matches!(next(), Stmt::Return(Some(_))));
        assert!(matches!(
            next(),
            Stmt::AugAssign {
                name: "n",
                op: BinOp::Add,
                ..
            }
        ));
        assert!(Stmt::Else.is_compound() && !Stmt::Pass.is_compound());
    }

    #[test]
    fn test_parse_def_errors() {
        let mut code = Code::new();
        for (src, err) in [
            ("def (x):", Error::InvalidSyntax),
            ("def f(1):", Error::InvalidSyntax),
            ("def f(a b):", Error::UnexpectedToken),
            ("def f(a)", Error::UnexpectedToken),
            ("def f(a, a):", Error::InvalidSyntax),
            ("def f(a, b, c, d, e, f, g, h, i):", Error::TooManyArguments),
            ("else", Error::UnexpectedToken),
            ("pass 1", Error::UnexpectedToken),
        ] {
            let mut parser = Parser::new(src).unwrap();
            assert_eq!(
                parser.parse_next_stmt(&mut code),
                Err(err),
                "source: {}",
                src
            );
        }
    }

    #[test]
    fn test_parse_imports_and_attributes() {
        use Item::{CallAttr, GetAttr, Ident, Int};
        let mut code = Code::new();
        let mut parser = Parser::new(
            "import display\nimport time as t, display as d\nfrom display import line, RED as R\n",
        )
        .unwrap();
        let Some(Stmt::Import { names, count }) = parser.parse_next_stmt(&mut code).unwrap() else {
            panic!("expected import");
        };
        assert_eq!(&names[..count], &[("display", "display")]);
        let Some(Stmt::Import { names, count }) = parser.parse_next_stmt(&mut code).unwrap() else {
            panic!("expected import");
        };
        assert_eq!(&names[..count], &[("time", "t"), ("display", "d")]);
        let Some(Stmt::FromImport {
            module,
            names,
            count,
        }) = parser.parse_next_stmt(&mut code).unwrap()
        else {
            panic!("expected from-import");
        };
        assert_eq!(
            (module, &names[..count]),
            ("display", &[("line", "line"), ("RED", "R")][..])
        );

        assert_rpn("x = d.WIDTH", &[Ident("d"), GetAttr("WIDTH")]);
        assert_rpn(
            "x = d.line(1, 2)",
            &[
                Ident("d"),
                Int(1),
                Int(2),
                CallAttr {
                    attr: "line",
                    argc: 2,
                    kw: 0,
                },
            ],
        );

        for (src, err) in [
            ("import", Error::InvalidSyntax),
            ("import 1", Error::InvalidSyntax),
            ("import a as", Error::InvalidSyntax),
            ("import a b", Error::UnexpectedToken),
            ("from a import", Error::InvalidSyntax),
            ("from a b", Error::UnexpectedToken),
            ("from a import *", Error::InvalidSyntax),
            ("x = a.", Error::InvalidSyntax),
            ("a.b c", Error::UnexpectedToken),
            ("import a, b, c, d, e, f, g, h, i", Error::TooManyArguments),
        ] {
            let mut parser = Parser::new(src).unwrap();
            assert_eq!(
                parser.parse_next_stmt(&mut code),
                Err(err),
                "source: {}",
                src
            );
        }
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

        for src in ["f(,)", "f(1,,)", "f(1", "x = 1 y", "f()(1 2)"] {
            let mut parser = Parser::new(src).unwrap();
            assert_eq!(
                parser.parse_next_stmt(&mut code),
                Err(Error::UnexpectedToken),
                "source: {}",
                src
            );
        }

        let mut p4 =
            Parser::new("f(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17)").unwrap();
        assert_eq!(p4.parse_next_stmt(&mut code), Err(Error::TooManyArguments));
    }

    #[test]
    fn test_parse_arithmetic_errors() {
        let mut code = Code::new();
        for src in [
            "x = 1 +",
            "x = (1 + 2",
            "x = 1 + * 2",
            "x = (,)",
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
