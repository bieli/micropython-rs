// ============================================================================
// STRINGS: literals with escapes, heap strings, str()/repr(), operators,
// indexing/slicing and string methods
// ============================================================================
//
// A string value is either `Value::Str` (a slice of the script source:
// literals without escapes and slices of them cost no memory) or
// `Value::HStr` (an object on the interpreter's heap, created at runtime).
//
// Rule for everything that allocates: the operands must still be on the
// value stack (or in variables), so a garbage collection triggered by the
// allocation sees them as roots. Indices are counted in characters (code
// points), like Python, not bytes.

use super::{Interpreter, Value};
use crate::heap::{Kind, ObjRef};
use crate::{Error, HardwareBridge, Module, Output};

/// Looks up a module by index (core modules first, then the bridge's).
pub(crate) type ModuleLookup<'m> = &'m dyn Fn(u8) -> Option<Module>;

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    /// The text of a string value.
    pub(super) fn str_of(&self, v: Value<'a>) -> Option<&str> {
        match v {
            Value::Str(s) => Some(s),
            Value::HStr(r) => Some(self.heap.str(r)),
            _ => None,
        }
    }

    pub(super) fn is_str(v: Value<'a>) -> bool {
        matches!(v, Value::Str(_) | Value::HStr(_))
    }

    /// Allocates a heap object, collecting garbage first if it doesn't fit.
    pub(super) fn alloc_obj(&mut self, kind: Kind, len: usize) -> Result<ObjRef, Error> {
        if let Some(r) = self.heap.alloc(kind, len) {
            return Ok(r);
        }
        self.collect();
        self.heap.alloc(kind, len).ok_or(Error::OutOfMemory)
    }

    /// Mark and sweep: roots are the variables and the value stack.
    pub(super) fn collect(&mut self) {
        self.heap.begin_mark();
        for &(_, v) in &self.vars[..self.nvars] {
            if let Value::HStr(r) | Value::HBytes(r) = v {
                self.heap.mark(r);
            }
        }
        for &v in &self.stack[..self.sp] {
            if let Value::HStr(r) | Value::HBytes(r) = v {
                self.heap.mark(r);
            }
        }
        self.heap.sweep();
    }

    /// A string literal: zero-copy unless it contains escape sequences.
    pub(super) fn literal(&mut self, raw: &'a str) -> Result<Value<'a>, Error> {
        if !raw.contains('\\') {
            return Ok(Value::Str(raw));
        }
        let mut len = 0;
        decode_escapes(raw, &mut |b| len += b.len())?;
        let r = self.alloc_obj(Kind::Str, len)?;
        let dst = self.heap.bytes_mut(r);
        let mut at = 0;
        decode_escapes(raw, &mut |b| {
            dst[at..at + b.len()].copy_from_slice(b);
            at += b.len();
        })?;
        Ok(Value::HStr(r))
    }

    /// A new heap string with the bytes of `parts` (strings) one after another.
    /// The bytes of a string or bytes value.
    fn raw_bytes(&self, v: Value<'a>) -> Option<&[u8]> {
        self.str_of(v)
            .map(str::as_bytes)
            .or_else(|| self.bytes_of(v))
    }

    /// A new heap string (or bytes, if `kind` says so) of `parts` one after another.
    fn concat_values(&mut self, kind: Kind, parts: &[Value<'a>]) -> Result<Value<'a>, Error> {
        let mut len = 0;
        for &p in parts {
            len += self.raw_bytes(p).ok_or(Error::TypeError)?.len();
        }
        let dst = self.alloc_obj(kind, len)?;
        let mut at = 0;
        for &p in parts {
            at += self.copy_into(p, 0, usize::MAX, dst, at);
        }
        Ok(if kind == Kind::Bytes {
            Value::HBytes(dst)
        } else {
            Value::HStr(dst)
        })
    }

    /// Copies bytes `from..to` (clamped) of string `src` into `dst` at `at`;
    /// returns the number of bytes copied.
    pub(super) fn copy_into(
        &mut self,
        src: Value<'a>,
        from: usize,
        to: usize,
        dst: ObjRef,
        at: usize,
    ) -> usize {
        match src {
            Value::Str(_) | Value::Bytes(_) => {
                let data: &[u8] = match src {
                    Value::Str(s) => s.as_bytes(),
                    Value::Bytes(b) => b,
                    _ => &[],
                };
                let part = &data[from..to.min(data.len())];
                self.heap.bytes_mut(dst)[at..at + part.len()].copy_from_slice(part);
                part.len()
            }
            Value::HStr(r) | Value::HBytes(r) => {
                let len = to.min(self.heap.bytes(r).len()) - from;
                self.heap.copy_obj(r, from, dst, at, len);
                len
            }
            _ => 0,
        }
    }

    /// Bytes `from..to` of a string as a new value (zero-copy for source strings).
    fn substring(&mut self, s: Value<'a>, from: usize, to: usize) -> Result<Value<'a>, Error> {
        match s {
            Value::Str(text) => Ok(Value::Str(&text[from..to])),
            Value::HStr(_) => {
                let dst = self.alloc_obj(Kind::Str, to - from)?;
                self.copy_into(s, from, to, dst, 0);
                Ok(Value::HStr(dst))
            }
            _ => Err(Error::TypeError),
        }
    }

    /// `+` and `*` with strings: `'a' + 'b'`, `'ab' * 3`, `3 * 'ab'`.
    pub(super) fn str_binop(
        &mut self,
        op: crate::ast_parser::BinOp,
        lhs: Value<'a>,
        rhs: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        use crate::ast_parser::BinOp;
        match op {
            BinOp::Add
                if (Self::is_str(lhs) && Self::is_str(rhs))
                    || (Self::is_bytes(lhs) && Self::is_bytes(rhs)) =>
            {
                if self.raw_bytes(rhs).is_some_and(<[u8]>::is_empty) {
                    return Ok(lhs);
                }
                if self.raw_bytes(lhs).is_some_and(<[u8]>::is_empty) {
                    return Ok(rhs);
                }
                let kind = if Self::is_bytes(lhs) {
                    Kind::Bytes
                } else {
                    Kind::Str
                };
                self.concat_values(kind, &[lhs, rhs])
            }
            BinOp::Mul => {
                let seq = |v: Value<'a>| Self::is_str(v) || Self::is_bytes(v);
                let (s, n) = match (lhs.as_int(), rhs.as_int()) {
                    (None, Some(n)) if seq(lhs) => (lhs, n),
                    (Some(n), None) if seq(rhs) => (rhs, n),
                    _ => return Err(Error::TypeError),
                };
                let unit = self.raw_bytes(s).map_or(0, <[u8]>::len);
                let kind = if Self::is_bytes(s) {
                    Kind::Bytes
                } else {
                    Kind::Str
                };
                if n <= 0 || unit == 0 {
                    return Ok(if kind == Kind::Bytes {
                        Value::Bytes(b"")
                    } else {
                        Value::Str("")
                    });
                }
                if n == 1 {
                    return Ok(s);
                }
                let total = unit
                    .checked_mul(n as usize)
                    .filter(|&t| t <= self.heap.capacity());
                let dst = self.alloc_obj(kind, total.ok_or(Error::OutOfMemory)?)?;
                for k in 0..n as usize {
                    self.copy_into(s, 0, usize::MAX, dst, k * unit);
                }
                Ok(if kind == Kind::Bytes {
                    Value::HBytes(dst)
                } else {
                    Value::HStr(dst)
                })
            }
            _ => Err(Error::TypeError),
        }
    }

    /// `s[i]`, with negative indices counting from the end.
    pub(super) fn index_value(
        &mut self,
        obj: Value<'a>,
        index: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        if Self::is_bytes(obj) {
            return self.bytes_index(obj, index);
        }
        let i = index.as_int().ok_or(Error::TypeError)?;
        let text = self.str_of(obj).ok_or(Error::TypeError)?;
        let n = text.chars().count() as i64;
        let i = if i < 0 { i + n } else { i };
        if i < 0 || i >= n {
            return Err(Error::IndexError);
        }
        let (from, ch) = text
            .char_indices()
            .nth(i as usize)
            .ok_or(Error::IndexError)?;
        let to = from + ch.len_utf8();
        self.substring(obj, from, to)
    }

    /// `s[a:b]` with Python's clamping (`None` means "from the start / to the end").
    pub(super) fn slice_value(
        &mut self,
        obj: Value<'a>,
        start: Value<'a>,
        end: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        let bound = |v: Value<'a>| match v {
            Value::None => Ok(None),
            other => other.as_int().map(Some).ok_or(Error::TypeError),
        };
        let (start, end) = (bound(start)?, bound(end)?);
        if Self::is_bytes(obj) {
            return self.bytes_slice(obj, start, end);
        }
        let text = self.str_of(obj).ok_or(Error::TypeError)?;
        let n = text.chars().count() as i64;
        let clamp = |i: i64| (if i < 0 { i + n } else { i }).clamp(0, n);
        let a = start.map_or(0, clamp);
        let b = end.map_or(n, clamp);
        if a >= b {
            return Ok(Value::Str(""));
        }
        let from = char_to_byte(text, a as usize);
        let to = char_to_byte(text, b as usize);
        self.substring(obj, from, to)
    }

    /// `str(v)` or `repr(v)` as a string value (`v` must be on the stack).
    pub(super) fn stringify(&mut self, v: Value<'a>, repr: bool) -> Result<Value<'a>, Error> {
        if Self::is_str(v) && !repr {
            return Ok(v); // strings are immutable: str(s) is s
        }
        if let Value::HBytes(src) = v {
            // str(b) and repr(b) are both the literal form b'...'
            let mut len = 0;
            super::bytes::write_bytes_repr(self.heap.bytes(src), &mut |p| len += p.len());
            let dst = self.alloc_obj(Kind::Str, len)?;
            let (data, out) = self.heap.src_dst(src, dst);
            let mut at = 0;
            super::bytes::write_bytes_repr(data, &mut |p| {
                out[at..at + p.len()].copy_from_slice(p.as_bytes());
                at += p.len();
            });
            return Ok(Value::HStr(dst));
        }
        if let Value::HStr(r) = v {
            let mut len = 0;
            write_str_repr(self.heap.str(r), &mut |p| len += p.len());
            let dst = self.alloc_obj(Kind::Str, len)?;
            let (src, out) = self.heap.src_dst(r, dst);
            let src = core::str::from_utf8(src).unwrap_or("");
            let mut at = 0;
            write_str_repr(src, &mut |p| {
                out[at..at + p.len()].copy_from_slice(p.as_bytes());
                at += p.len();
            });
            return Ok(Value::HStr(dst));
        }
        // Everything else is rendered without reading the heap
        let mut len = 0;
        {
            let hw_modules: &[Module] = match &self.hw {
                Some(hw) => hw.modules(),
                None => &[],
            };
            let lookup = |i: u8| super::lookup_module(hw_modules, i);
            write_plain(v, &lookup, repr, &mut |p| len += p.len());
        }
        let dst = self.alloc_obj(Kind::Str, len)?;
        let hw_modules: &[Module] = match &self.hw {
            Some(hw) => hw.modules(),
            None => &[],
        };
        let lookup = |i: u8| super::lookup_module(hw_modules, i);
        let out = self.heap.bytes_mut(dst);
        let mut at = 0;
        write_plain(v, &lookup, repr, &mut |p| {
            out[at..at + p.len()].copy_from_slice(p.as_bytes());
            at += p.len();
        });
        Ok(Value::HStr(dst))
    }

    /// Writes `v` as `print()` shows it (`repr == false`) or as the REPL does.
    pub(super) fn write_value(&mut self, v: Value<'_>, repr: bool) {
        let out = &mut *self.out;
        match v {
            Value::HStr(r) => {
                let s = self.heap.str(r);
                if repr {
                    write_str_repr(s, &mut |p| out.write_str(p));
                } else {
                    out.write_str(s);
                }
            }
            Value::HBytes(r) => {
                // print(b) shows the literal form too, like Python
                super::bytes::write_bytes_repr(self.heap.bytes(r), &mut |p| out.write_str(p));
            }
            other => {
                let hw_modules: &[Module] = match &self.hw {
                    Some(hw) => hw.modules(),
                    None => &[],
                };
                let lookup = |i: u8| super::lookup_module(hw_modules, i);
                write_plain(other, &lookup, repr, &mut |p| out.write_str(p));
            }
        }
    }

    /// Built-in functions on strings; `None` if `name` isn't one of them.
    pub(super) fn str_builtin(
        &mut self,
        name: &str,
        args: &[Value<'a>],
    ) -> Option<Result<Value<'a>, Error>> {
        if !matches!(
            name,
            "len" | "str" | "repr" | "int" | "ord" | "chr" | "bytes"
        ) {
            return None;
        }
        Some(self.str_builtin_call(name, args))
    }

    fn str_builtin_call(&mut self, name: &str, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        match (name, args) {
            ("len", [v]) => match (self.str_of(*v), self.bytes_of(*v)) {
                (Some(s), _) => Ok(Value::Int(s.chars().count() as i64)),
                (_, Some(b)) => Ok(Value::Int(b.len() as i64)),
                _ => Err(Error::TypeError),
            },
            ("bytes", _) => self.bytes_builtin(args),
            ("str", []) => Ok(Value::Str("")),
            ("str", [v]) => self.stringify(*v, false),
            ("repr", [v]) => self.stringify(*v, true),
            ("int", [v]) => match v.as_int() {
                Some(n) => Ok(Value::Int(n)),
                None => match self.str_of(*v) {
                    Some(s) => parse_int(s, 10).map(Value::Int),
                    None => Err(Error::TypeError),
                },
            },
            ("int", [v, base]) => {
                let base = base.as_int().ok_or(Error::TypeError)?;
                match self.str_of(*v) {
                    Some(s) if base == 0 || (2..=36).contains(&base) => {
                        parse_int(s, base as u32).map(Value::Int)
                    }
                    Some(_) => Err(Error::ValueError),
                    None => Err(Error::TypeError),
                }
            }
            ("ord", [v]) => match self.str_of(*v) {
                Some(s) => {
                    let mut chars = s.chars();
                    match (chars.next(), chars.next()) {
                        (Some(c), None) => Ok(Value::Int(c as i64)),
                        _ => Err(Error::TypeError),
                    }
                }
                None => Err(Error::TypeError),
            },
            ("chr", [v]) => {
                let code = v.as_int().ok_or(Error::TypeError)?;
                let ch = u32::try_from(code)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or(Error::ValueError)?;
                let mut buf = [0u8; 4];
                let text = ch.encode_utf8(&mut buf);
                let dst = self.alloc_obj(Kind::Str, text.len())?;
                self.heap.bytes_mut(dst).copy_from_slice(text.as_bytes());
                Ok(Value::HStr(dst))
            }
            _ => Err(Error::TypeError),
        }
    }

    /// `s.method(args)` for strings.
    pub(super) fn str_method(
        &mut self,
        s: Value<'a>,
        name: &str,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        let text = self.str_of(s).ok_or(Error::TypeError)?;
        match (name, args.len()) {
            ("encode", 0 | 1) => self.encode(s),
            ("startswith", 1) => Ok(Value::Bool(text.starts_with(self.arg_str(args, 0)?))),
            ("endswith", 1) => Ok(Value::Bool(text.ends_with(self.arg_str(args, 0)?))),
            ("find", 1) => {
                let needle = self.arg_str(args, 0)?;
                Ok(Value::Int(match text.find(needle) {
                    Some(at) => text[..at].chars().count() as i64,
                    None => -1,
                }))
            }
            ("count", 1) => {
                let needle = self.arg_str(args, 0)?;
                Ok(Value::Int(if needle.is_empty() {
                    text.chars().count() as i64 + 1
                } else {
                    text.matches(needle).count() as i64
                }))
            }
            ("isdigit" | "isalpha" | "isspace" | "isalnum" | "isupper" | "islower", 0) => {
                let test: fn(char) -> bool = match name {
                    "isdigit" => |c| c.is_ascii_digit(),
                    "isalpha" => char::is_alphabetic,
                    "isspace" => char::is_whitespace,
                    "isalnum" => char::is_alphanumeric,
                    "isupper" => |c| !c.is_lowercase(),
                    _ => |c| !c.is_uppercase(),
                };
                let cased = match name {
                    "isupper" => text.chars().any(char::is_uppercase),
                    "islower" => text.chars().any(char::is_lowercase),
                    _ => true,
                };
                Ok(Value::Bool(
                    !text.is_empty() && cased && text.chars().all(test),
                ))
            }
            ("strip" | "lstrip" | "rstrip", 0 | 1) => {
                let chars = if args.is_empty() {
                    None
                } else {
                    Some(self.arg_str(args, 0)?)
                };
                let keep = |c: char| match chars {
                    Some(set) => !set.contains(c),
                    None => !c.is_whitespace(),
                };
                let from = if name == "rstrip" {
                    0
                } else {
                    text.find(keep).unwrap_or(text.len())
                };
                let to = if name == "lstrip" {
                    text.len()
                } else {
                    text.rfind(keep).map_or(from, |i| {
                        i + text[i..].chars().next().map_or(1, char::len_utf8)
                    })
                };
                let (from, to) = (from, to.max(from));
                self.substring(s, from, to)
            }
            ("upper" | "lower", 0) => {
                let upper = name == "upper";
                let convert = |c: char, sink: &mut dyn FnMut(char)| {
                    if upper {
                        c.to_uppercase().for_each(&mut *sink)
                    } else {
                        c.to_lowercase().for_each(&mut *sink)
                    }
                };
                let mut len = 0;
                for c in text.chars() {
                    convert(c, &mut |u| len += u.len_utf8());
                }
                let dst = self.alloc_obj(Kind::Str, len)?;
                let mut buf = [0u8; 4];
                let mut at = 0;
                let mut offset = 0;
                // Re-read the source after the allocation (it may have moved)
                while let Some(c) = self.str_of(s).and_then(|t| t[offset..].chars().next()) {
                    offset += c.len_utf8();
                    let out = self.heap.bytes_mut(dst);
                    convert(c, &mut |u| {
                        let e = u.encode_utf8(&mut buf);
                        out[at..at + e.len()].copy_from_slice(e.as_bytes());
                        at += e.len();
                    });
                }
                Ok(Value::HStr(dst))
            }
            ("replace", 2) => {
                let (old, new) = (args[0], args[1]);
                let old_text = self.arg_str(args, 0)?;
                let new_len = self.arg_str(args, 1)?.len();
                if old_text.is_empty() {
                    return self.replace_empty(s, new);
                }
                let hits = text.matches(old_text).count();
                if hits == 0 {
                    return Ok(s);
                }
                let old_len = old_text.len();
                let len = text.len() - hits * old_len + hits * new_len;
                let dst = self.alloc_obj(Kind::Str, len)?;
                let (mut pos, mut at) = (0, 0);
                loop {
                    // Find the next match in the (possibly moved) source
                    let next = {
                        let text = self.str_of(s).unwrap_or("");
                        let old_text = self.str_of(old).unwrap_or("");
                        text[pos..].find(old_text).map(|i| pos + i)
                    };
                    match next {
                        Some(i) => {
                            at += self.copy_into(s, pos, i, dst, at);
                            at += self.copy_into(new, 0, usize::MAX, dst, at);
                            pos = i + old_len;
                        }
                        None => {
                            self.copy_into(s, pos, usize::MAX, dst, at);
                            break;
                        }
                    }
                }
                Ok(Value::HStr(dst))
            }
            _ => Err(Error::AttributeError),
        }
    }

    /// String argument number `i` of a method call.
    fn arg_str(&self, args: &[Value<'a>], i: usize) -> Result<&str, Error> {
        args.get(i)
            .and_then(|&v| self.str_of(v))
            .ok_or(Error::TypeError)
    }

    /// `s.replace('', new)`: `new` before every character and at the end.
    fn replace_empty(&mut self, s: Value<'a>, new: Value<'a>) -> Result<Value<'a>, Error> {
        let text = self.str_of(s).unwrap_or("");
        let n = text.chars().count();
        let new_len = self.str_of(new).map_or(0, str::len);
        let dst = self.alloc_obj(Kind::Str, text.len() + (n + 1) * new_len)?;
        let (mut pos, mut at) = (0, 0);
        for _ in 0..n {
            at += self.copy_into(new, 0, usize::MAX, dst, at);
            let w = self
                .str_of(s)
                .and_then(|t| t[pos..].chars().next())
                .map_or(0, char::len_utf8);
            at += self.copy_into(s, pos, pos + w, dst, at);
            pos += w;
        }
        self.copy_into(new, 0, usize::MAX, dst, at);
        Ok(Value::HStr(dst))
    }
}

/// Byte offset of character number `chars` (or the end).
fn char_to_byte(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i)
}

/// Decodes Python escape sequences in a string literal, emitting UTF-8 bytes.
pub(crate) fn decode_escapes(raw: &str, sink: &mut dyn FnMut(&[u8])) -> Result<(), Error> {
    let mut chars = raw.chars();
    let mut buf = [0u8; 4];
    let mut emit = |c: char, sink: &mut dyn FnMut(&[u8])| sink(c.encode_utf8(&mut buf).as_bytes());
    while let Some(c) = chars.next() {
        if c != '\\' {
            emit(c, sink);
            continue;
        }
        let Some(e) = chars.next() else {
            emit('\\', sink);
            break;
        };
        let simple = match e {
            '\\' => Some('\\'),
            '\'' => Some('\''),
            '"' => Some('"'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            'a' => Some('\x07'),
            'b' => Some('\x08'),
            'f' => Some('\x0c'),
            'v' => Some('\x0b'),
            _ => None,
        };
        if let Some(ch) = simple {
            emit(ch, sink);
            continue;
        }
        let code = match e {
            'x' | 'u' | 'U' => {
                let digits = match e {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let mut code = 0u32;
                for _ in 0..digits {
                    let d = chars
                        .next()
                        .and_then(|c| c.to_digit(16))
                        .ok_or(Error::InvalidSyntax)?;
                    code = code * 16 + d;
                }
                code
            }
            '0'..='7' => {
                // Up to three octal digits
                let mut code = e.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    let rest = chars.as_str();
                    match rest.chars().next().and_then(|c| c.to_digit(8)) {
                        Some(d) => {
                            code = code * 8 + d;
                            chars.next();
                        }
                        None => break,
                    }
                }
                code
            }
            other => {
                // Unknown escapes stay as written, like Python
                emit('\\', sink);
                emit(other, sink);
                continue;
            }
        };
        emit(char::from_u32(code).ok_or(Error::InvalidSyntax)?, sink);
    }
    Ok(())
}

/// Writes `s` as a Python string literal (what `repr()` returns).
pub(crate) fn write_str_repr(s: &str, sink: &mut dyn FnMut(&str)) {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut buf = [0u8; 4];
    sink(quote.encode_utf8(&mut buf));
    for c in s.chars() {
        match c {
            '\\' => sink("\\\\"),
            '\n' => sink("\\n"),
            '\r' => sink("\\r"),
            '\t' => sink("\\t"),
            c if c == quote => sink(if quote == '\'' { "\\'" } else { "\\\"" }),
            c if (c as u32) < 0x20 || (0x7F..0xA0).contains(&(c as u32)) => {
                let hex = b"0123456789abcdef";
                let code = c as u32 as usize;
                let esc = [b'\\', b'x', hex[code >> 4], hex[code & 0xF]];
                sink(core::str::from_utf8(&esc).unwrap_or(""));
            }
            c => sink(c.encode_utf8(&mut buf)),
        }
    }
    sink(quote.encode_utf8(&mut buf));
}

/// Writes a value that isn't a heap string; `repr` quotes source strings.
pub(crate) fn write_plain(
    v: Value<'_>,
    modules: ModuleLookup<'_>,
    repr: bool,
    sink: &mut dyn FnMut(&str),
) {
    match v {
        Value::None => sink("None"),
        Value::Bool(true) => sink("True"),
        Value::Bool(false) => sink("False"),
        Value::Int(n) => {
            let mut buf = [0u8; 20];
            sink(format_i64(n, &mut buf));
        }
        Value::Str(s) => {
            if repr {
                write_str_repr(s, sink)
            } else {
                sink(s)
            }
        }
        Value::Bytes(b) => super::bytes::write_bytes_repr(b, sink),
        Value::HStr(_) | Value::HBytes(_) => sink("?"),
        Value::Module(m) => {
            let name = modules(m).map_or("?", |m| m.name);
            for part in ["<module '", name, "'>"] {
                sink(part);
            }
        }
        Value::NativeFn { module, func } => {
            let (m, f) = modules(module).map_or(("?", "?"), |m| {
                (
                    m.name,
                    m.functions.get(func as usize).map_or("?", |f| f.name),
                )
            });
            for part in ["<function ", m, ".", f, ">"] {
                sink(part);
            }
        }
    }
}

/// Decimal text of `n` (20 bytes fit any i64).
pub(crate) fn format_i64(n: i64, buf: &mut [u8; 20]) -> &str {
    let mut idx = buf.len();
    let mut v = n.unsigned_abs();
    loop {
        idx -= 1;
        buf[idx] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if n < 0 {
        idx -= 1;
        buf[idx] = b'-';
    }
    core::str::from_utf8(&buf[idx..]).unwrap_or("?")
}

/// `int(text, base)` with Python's rules: surrounding whitespace, a sign,
/// `_` between digits, and `0x`/`0o`/`0b` prefixes (required for base 0).
pub(crate) fn parse_int(text: &str, base: u32) -> Result<i64, Error> {
    let t = text.trim();
    let (negative, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let prefix = |p: &str| t.len() > 2 && t[..2].eq_ignore_ascii_case(p);
    let (base, digits) = match base {
        0 | 16 if prefix("0x") => (16, &t[2..]),
        0 | 8 if prefix("0o") => (8, &t[2..]),
        0 | 2 if prefix("0b") => (2, &t[2..]),
        0 => (10, t),
        b => (b, t),
    };
    let digits = digits
        .strip_prefix('_')
        .filter(|_| base != 10)
        .unwrap_or(digits);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return Err(Error::ValueError);
    }
    let mut value: i64 = 0;
    for c in digits.chars().filter(|&c| c != '_') {
        let d = c.to_digit(base).ok_or(Error::ValueError)? as i64;
        value = value
            .checked_mul(base as i64)
            .and_then(|v| {
                if negative {
                    v.checked_sub(d)
                } else {
                    v.checked_add(d)
                }
            })
            .ok_or(Error::Overflow)?;
    }
    Ok(value)
}
