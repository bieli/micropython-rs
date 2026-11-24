// ============================================================================
// STRING FORMATTING: `'%d' % x`, `'{:>5}'.format(x)` and f-strings
// ============================================================================
//
// All three use one format-spec engine (`Spec` + `Interpreter::format_one`),
// with Python's mini-language: `[[fill]align][sign][#][0][width][,][.prec][type]`.
// Types: `d x X o b c n` for integers, `s` for strings, and `f F e E g G %`
// for floats (and ints, as floats). Without the `float` feature, `f %` format
// integers exactly (`'{:.2f}'.format(3)` is `3.00`).
//
// A formatted string is built from pieces (literal parts and formatted
// values) kept on the value stack, so they're rooted, then joined with one
// allocation.

use super::{Interpreter, Kwargs, Value};
use crate::heap::Kind;
use crate::{Error, HardwareBridge, Output};

/// A parsed format specification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Spec {
    fill: char,
    /// `<`, `>`, `^` or `=` (`None`: the default for the value's type)
    align: Option<char>,
    /// `-`, `+` or ` `
    sign: char,
    alt: bool,
    width: usize,
    /// Thousands separator (`,` or `_`), if any
    group: Option<char>,
    precision: Option<usize>,
    /// The type letter, or `\0` for none
    ty: char,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            fill: ' ',
            align: None,
            sign: '-',
            alt: false,
            width: 0,
            group: None,
            precision: None,
            ty: '\0',
        }
    }
}

/// Longest number text (64 binary digits, separators, prefix, `.` and up
/// to 64 decimals).
const NUM_BUF: usize = 192;
const MAX_PRECISION: usize = 64;

impl Spec {
    /// Parses the part after `:` in `{...}` (format()'s mini-language).
    pub(super) fn parse(text: &str) -> Result<Self, Error> {
        let mut spec = Spec::default();
        let chars: [Option<char>; 2] = {
            let mut it = text.chars();
            [it.next(), it.next()]
        };
        let mut rest = text;
        let is_align = |c: char| matches!(c, '<' | '>' | '^' | '=');
        match chars {
            [Some(f), Some(a)] if is_align(a) => {
                spec.fill = f;
                spec.align = Some(a);
                rest = &text[f.len_utf8() + 1..];
            }
            [Some(a), _] if is_align(a) => {
                spec.align = Some(a);
                rest = &text[1..];
            }
            _ => {}
        }
        let mut bytes = rest.as_bytes();
        if let Some(&s @ (b'+' | b'-' | b' ')) = bytes.first() {
            spec.sign = s as char;
            bytes = &bytes[1..];
        }
        if bytes.first() == Some(&b'#') {
            spec.alt = true;
            bytes = &bytes[1..];
        }
        if bytes.first() == Some(&b'0') {
            // `0` before the width: zero padding after the sign
            if spec.align.is_none() {
                spec.fill = '0';
                spec.align = Some('=');
            }
            bytes = &bytes[1..];
        }
        let (width, used) = digits(bytes);
        spec.width = width;
        bytes = &bytes[used..];
        if let Some(&g @ (b',' | b'_')) = bytes.first() {
            spec.group = Some(g as char);
            bytes = &bytes[1..];
        }
        if bytes.first() == Some(&b'.') {
            let (precision, used) = digits(&bytes[1..]);
            if used == 0 {
                return Err(Error::ValueError);
            }
            spec.precision = Some(precision.min(MAX_PRECISION));
            bytes = &bytes[1 + used..];
        }
        match bytes {
            [] => {}
            [t] if b"sdxXobcnf%Fr".contains(t) => spec.ty = *t as char,
            [t] if cfg!(feature = "float") && b"eEgG".contains(t) => spec.ty = *t as char,
            _ => return Err(Error::ValueError), // invalid format specifier
        }
        Ok(spec)
    }
}

/// A decimal number at the start of `bytes`: `(value, digits used)`.
fn digits(bytes: &[u8]) -> (usize, usize) {
    let used = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    let value = bytes[..used].iter().fold(0usize, |n, &d| {
        n.saturating_mul(10).saturating_add((d - b'0') as usize)
    });
    (value.min(4096), used)
}

/// Writes the text of an integer for `spec` into `buf`; returns
/// `(sign and prefix length, total length)`.
fn format_int(
    n: i64,
    spec: &Spec,
    min_digits: usize,
    buf: &mut [u8; NUM_BUF],
) -> Result<(usize, usize), Error> {
    let (radix, upper, prefix): (u64, bool, &[u8]) = match spec.ty {
        'x' => (16, false, b"0x"),
        'X' => (16, true, b"0X"),
        'o' => (8, false, b"0o"),
        'b' => (2, false, b"0b"),
        _ => (10, false, b""),
    };
    let mut len = 0;
    let mut put = |b: u8, len: &mut usize| {
        buf[*len] = b;
        *len += 1;
    };
    match (n < 0, spec.sign) {
        (true, _) => put(b'-', &mut len),
        (false, '+') => put(b'+', &mut len),
        (false, ' ') => put(b' ', &mut len),
        _ => {}
    }
    if spec.alt {
        for &b in prefix {
            put(b, &mut len);
        }
    }
    let head = len;
    let scaled = if spec.ty == '%' {
        n.unsigned_abs().saturating_mul(100)
    } else {
        n.unsigned_abs()
    };
    // Digits, least significant first, then reversed in place
    let mut d = [0u8; 64];
    let mut nd = 0;
    let mut v = scaled;
    loop {
        let digit = (v % radix) as u8;
        d[nd] = match digit {
            0..=9 => b'0' + digit,
            _ if upper => b'A' + digit - 10,
            _ => b'a' + digit - 10,
        };
        nd += 1;
        v /= radix;
        if v == 0 {
            break;
        }
    }
    while nd < min_digits.min(64) {
        d[nd] = b'0';
        nd += 1;
    }
    let group_every = if radix == 10 { 3 } else { 4 };
    for i in (0..nd).rev() {
        put(d[i], &mut len);
        if let Some(g) = spec.group {
            if i > 0 && i % group_every == 0 {
                put(g as u8, &mut len);
            }
        }
    }
    if matches!(spec.ty, 'f' | 'F' | '%') {
        let decimals = spec.precision.unwrap_or(6);
        if decimals > 0 || spec.alt {
            put(b'.', &mut len);
        }
        for _ in 0..decimals {
            put(b'0', &mut len);
        }
        if spec.ty == '%' {
            put(b'%', &mut len);
        }
    } else if spec.precision.is_some() && min_digits == 0 {
        return Err(Error::ValueError); // precision not allowed for integers
    }
    Ok((head, len))
}

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    /// Formats one value (rooted) with a conversion (`!r`, `!s`) and a spec;
    /// returns a string value. `min_digits` is `%`-formatting's precision
    /// for integers (`'%.3d' % 5` is `005`).
    #[inline(never)]
    pub(super) fn format_one(
        &mut self,
        v: Value<'a>,
        conv: char,
        spec: &Spec,
        min_digits: usize,
    ) -> Result<Value<'a>, Error> {
        let v = match conv {
            'r' | 'a' => self.stringify(v, true)?,
            's' => self.stringify(v, false)?,
            _ => v,
        };
        let numeric = matches!(spec.ty, 'd' | 'x' | 'X' | 'o' | 'b' | 'n' | 'f' | 'F' | '%')
            || spec.ty == '\0' && matches!(v, Value::Int(_))
            || spec.ty == '\0' && matches!(v, Value::Bool(_)) && *spec != Spec::default();
        if spec.ty == 'c' {
            // A character from its code (or a one-character string)
            let text = match v.as_int() {
                Some(code) => {
                    let ch = char::from_u32(code as u32).ok_or(Error::Overflow)?;
                    let mut buf = [0u8; 4];
                    let ch = ch.encode_utf8(&mut buf);
                    let r = self.alloc_obj(Kind::Str, ch.len())?;
                    self.heap.bytes_mut(r).copy_from_slice(ch.as_bytes());
                    Value::HStr(r)
                }
                None if self.str_of(v).is_some_and(|s| s.chars().count() == 1) => v,
                None => return Err(Error::TypeError),
            };
            let base = self.sp;
            self.push(text)?;
            let result = self.padded_text(text, spec);
            self.sp = base;
            return result;
        }
        if cfg!(feature = "float") {
            let float_type = matches!(spec.ty, 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%');
            let is_float = matches!(v, Value::Float(_));
            if is_float && !float_type && !matches!(spec.ty, '\0' | 'n') {
                return Err(Error::ValueError); // `'{:d}'.format(1.5)`
            }
            if is_float || float_type {
                let x = super::floats::as_float(v).ok_or(Error::ValueError)?;
                return self.format_float_spec(crate::float_to_f64(x), spec);
            }
        }
        if numeric {
            // `'{:d}'.format('s')`: unknown format code for the type
            let n = v.as_int().ok_or(Error::ValueError)?;
            let mut buf = [0u8; NUM_BUF];
            let (head, len) = format_int(n, spec, min_digits, &mut buf)?;
            return self.padded_number(&buf[..len], head, spec);
        }
        if *spec == Spec::default() {
            return self.stringify(v, false);
        }
        if !matches!(spec.ty, 's' | '\0' | 'r') || spec.group.is_some() {
            return Err(Error::ValueError);
        }
        let text = if Self::is_str(v) {
            v
        } else {
            self.stringify(v, false)?
        };
        let base = self.sp;
        self.push(text)?;
        let result = self.padded_text(text, spec);
        self.sp = base;
        result
    }

    /// A float (or an int, for a float type) formatted with a spec: sign,
    /// digits (with `,` / `_` groups), then padding.
    #[inline(never)]
    fn format_float_spec(&mut self, x: f64, spec: &Spec) -> Result<Value<'a>, Error> {
        let mut digits = super::floats::Buf::<512>::new();
        let ty = if spec.ty == 'n' { 'g' } else { spec.ty };
        super::floats::format_float(x, ty, spec.precision, spec.alt, &mut digits)?;
        let digits = digits.as_str().as_bytes();
        let mut text = [0u8; NUM_BUF * 4];
        let mut len = 0;
        let mut put = |b: u8, len: &mut usize| -> Result<(), Error> {
            *text.get_mut(*len).ok_or(Error::Overflow)? = b;
            *len += 1;
            Ok(())
        };
        match (x.is_sign_negative() && !x.is_nan(), spec.sign) {
            (true, _) => put(b'-', &mut len)?,
            (false, '+') => put(b'+', &mut len)?,
            (false, ' ') => put(b' ', &mut len)?,
            _ => {}
        }
        let head = len;
        // Groups in the integer part: the digits before `.`, `e` or the end
        let int_digits = digits.iter().take_while(|b| b.is_ascii_digit()).count();
        for (i, &b) in digits.iter().enumerate() {
            put(b, &mut len)?;
            if let Some(g) = spec.group {
                let left = int_digits.saturating_sub(i + 1);
                if i < int_digits && left > 0 && left % 3 == 0 {
                    put(g as u8, &mut len)?;
                }
            }
        }
        self.padded_number(&text[..len], head, spec)
    }

    /// Pads a number's text (`head` bytes of sign and prefix first).
    fn padded_number(&mut self, text: &[u8], head: usize, spec: &Spec) -> Result<Value<'a>, Error> {
        let len = text.len();
        let pad = spec.width.saturating_sub(len);
        let mut fill = [0u8; 4];
        let fill = spec.fill.encode_utf8(&mut fill).as_bytes();
        let (left, right) = match spec.align.unwrap_or('>') {
            '<' => (0, pad),
            '^' => (pad / 2, pad - pad / 2),
            _ => (pad, 0),
        };
        let r = self.alloc_obj(Kind::Str, len + pad * fill.len())?;
        let out = self.heap.bytes_mut(r);
        let mut at = 0;
        let mut put = |bytes: &[u8], at: &mut usize| {
            out[*at..*at + bytes.len()].copy_from_slice(bytes);
            *at += bytes.len();
        };
        if spec.align == Some('=') {
            // Padding between the sign/prefix and the digits: `-0042`
            put(&text[..head], &mut at);
            for _ in 0..pad {
                put(fill, &mut at);
            }
            put(&text[head..], &mut at);
        } else {
            for _ in 0..left {
                put(fill, &mut at);
            }
            put(text, &mut at);
            for _ in 0..right {
                put(fill, &mut at);
            }
        }
        Ok(Value::HStr(r))
    }

    /// Pads a string (`text`, rooted), cut to `precision` characters.
    fn padded_text(&mut self, text: Value<'a>, spec: &Spec) -> Result<Value<'a>, Error> {
        if spec.align == Some('=') {
            return Err(Error::ValueError); // `=` alignment is only for numbers
        }
        let (chars, bytes, all) = {
            let s = self.str_of(text).unwrap_or("");
            let chars = s.chars().count();
            let keep = spec.precision.map_or(chars, |p| p.min(chars));
            let bytes = s.char_indices().nth(keep).map_or(s.len(), |(i, _)| i);
            (keep, bytes, s.len())
        };
        let pad = spec.width.saturating_sub(chars);
        if pad == 0 && bytes == all {
            return Ok(text);
        }
        let mut fill = [0u8; 4];
        let fill = spec.fill.encode_utf8(&mut fill);
        let fill_len = fill.len();
        let (left, right) = match spec.align.unwrap_or('<') {
            '>' => (pad, 0),
            '^' => (pad / 2, pad - pad / 2),
            _ => (0, pad),
        };
        let r = self.alloc_obj(Kind::Str, bytes + pad * fill_len)?;
        let mut at = 0;
        for _ in 0..left {
            self.heap.bytes_mut(r)[at..at + fill_len].copy_from_slice(fill.as_bytes());
            at += fill_len;
        }
        at += self.copy_into(text, 0, bytes, r, at);
        for _ in 0..right {
            self.heap.bytes_mut(r)[at..at + fill_len].copy_from_slice(fill.as_bytes());
            at += fill_len;
        }
        Ok(Value::HStr(r))
    }

    /// Joins the string values `stack[base..]` into one (and drops them).
    pub(super) fn join_pieces(&mut self, base: usize) -> Result<Value<'a>, Error> {
        let n = self.sp - base;
        let result = match n {
            0 => Ok(Value::Str("")),
            1 => Ok(self.stack[base]),
            _ => self.concat_stack(base),
        };
        self.sp = base;
        result
    }

    /// `fmt % args` (`fmt` and `args` rooted).
    #[inline(never)]
    pub(super) fn percent_format(
        &mut self,
        fmt: Value<'a>,
        args: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        let base = self.sp;
        let result = self.percent_pieces(fmt, args, base);
        match result {
            Ok(()) => self.join_pieces(base),
            Err(e) => {
                self.sp = base;
                Err(e)
            }
        }
    }

    fn percent_pieces(
        &mut self,
        fmt: Value<'a>,
        args: Value<'a>,
        base: usize,
    ) -> Result<(), Error> {
        // A tuple gives the values; a dict serves `%(name)s`; anything else is one value
        let tuple = match self.obj_kind(args) {
            Some(Kind::Tuple) => match args {
                Value::Obj(r) => Some(r),
                _ => None,
            },
            _ => None,
        };
        let is_dict = self.obj_kind(args) == Some(Kind::Dict);
        let nargs = tuple.map_or(1, |r| self.count(r));
        let mut next_arg = 0;
        let mut pos = 0;
        let mut used_dict = false;
        loop {
            // Positions are re-read each time: an allocation may move `fmt`
            let text = self.str_of(fmt).unwrap_or("");
            let Some(k) = text[pos..].find('%') else {
                let rest = self.substring(fmt, pos, text.len())?;
                self.push(rest)?;
                break;
            };
            let start = pos + k;
            if start > pos {
                let lit = self.substring(fmt, pos, start)?;
                self.push(lit)?;
            }
            let text = self.str_of(fmt).unwrap_or("");
            let bytes = text.as_bytes();
            let mut i = start + 1;
            // `%(name)s`
            let mut key: Option<(usize, usize)> = None;
            if bytes.get(i) == Some(&b'(') {
                let close = text[i..].find(')').ok_or(Error::ValueError)? + i;
                key = Some((i + 1, close));
                i = close + 1;
            }
            let mut spec = Spec::default();
            while let Some(&f) = bytes.get(i) {
                match f {
                    b'-' => spec.align = Some('<'),
                    b'+' => spec.sign = '+',
                    b' ' if spec.sign != '+' => spec.sign = ' ',
                    b'#' => spec.alt = true,
                    b'0' => {
                        if spec.align.is_none() {
                            spec.fill = '0';
                            spec.align = Some('=');
                        }
                    }
                    _ => break,
                }
                i += 1;
            }
            let (width, used) = digits(&bytes[i..]);
            spec.width = width;
            i += used;
            let mut min_digits = 0;
            if bytes.get(i) == Some(&b'.') {
                let (p, used) = digits(&bytes[i + 1..]);
                spec.precision = Some(p.min(MAX_PRECISION));
                min_digits = p.min(MAX_PRECISION);
                i += 1 + used;
            }
            if spec.align == Some('<') && spec.fill == '0' {
                spec.fill = ' '; // `-` wins over `0`
            }
            // `%10s` pads on the left (unlike `format()`), unless `-` is given
            spec.align = spec.align.or(Some('>'));
            let conv = *bytes.get(i).ok_or(Error::ValueError)? as char;
            i += 1;
            pos = i;
            if conv == '%' {
                self.push(Value::Str("%"))?;
                continue;
            }
            let value = match key {
                Some((a, b)) => {
                    if !is_dict {
                        return Err(Error::TypeError); // format requires a mapping
                    }
                    used_dict = true;
                    let name = self.substring(fmt, a, b)?;
                    self.push(name)?;
                    let v = self.index_value(args, name)?;
                    self.sp -= 1;
                    v
                }
                None => {
                    if next_arg >= nargs {
                        return Err(Error::TypeError); // not enough arguments
                    }
                    next_arg += 1;
                    match tuple {
                        Some(r) => self.slot(r, next_arg - 1),
                        None => args,
                    }
                }
            };
            let (convert, ty, digits_for_int) = match conv {
                's' => ('s', '\0', 0),
                'r' | 'a' => ('r', '\0', 0),
                'd' | 'i' | 'u' => ('\0', 'd', min_digits),
                'x' | 'X' | 'o' => ('\0', conv, min_digits),
                'c' => ('\0', 'c', 0),
                'f' | 'F' if !cfg!(feature = "float") => ('\0', 'f', 0),
                'f' | 'F' | 'e' | 'E' | 'g' | 'G' if cfg!(feature = "float") => ('\0', conv, 0),
                _ => return Err(Error::ValueError), // unsupported format character
            };
            // `'%d' % 3.7` is `3` (towards zero), `'%x' % 1.5` an error
            let value = match (ty, value) {
                ('d', Value::Float(f)) => Value::Int(super::floats::float_to_int(f)?),
                _ => value,
            };
            let float_conv = matches!(ty, 'f' | 'F' | 'e' | 'E' | 'g' | 'G');
            if matches!(ty, 'd' | 'x' | 'X' | 'o') && value.as_int().is_none()
                || float_conv && value.as_int().is_none() && !matches!(value, Value::Float(_))
            {
                return Err(Error::TypeError); // `%d` needs a number
            }
            spec.ty = ty;
            if !float_conv && digits_for_int > 0 {
                spec.precision = None;
            }
            if matches!(conv, 's' | 'r' | 'a') {
                // `%.3s` cuts the text; the value is converted first
                spec.ty = 's';
            }
            let piece = self.format_one(value, convert, &spec, digits_for_int)?;
            self.push(piece)?;
        }
        let consumed_all =
            tuple.map_or(next_arg == 1 || used_dict || is_dict, |_| next_arg == nargs);
        if !consumed_all {
            return Err(Error::TypeError); // not all arguments converted
        }
        let _ = base;
        Ok(())
    }

    /// `fmt.format(*args, **kwargs)` (`fmt` and the arguments rooted).
    #[inline(never)]
    pub(super) fn str_format(
        &mut self,
        fmt: Value<'a>,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        let base = self.sp;
        let result = self.format_pieces(fmt, args, kwargs);
        match result {
            Ok(()) => self.join_pieces(base),
            Err(e) => {
                self.sp = base;
                Err(e)
            }
        }
    }

    fn format_pieces(
        &mut self,
        fmt: Value<'a>,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Result<(), Error> {
        let mut pos = 0;
        let mut auto = 0;
        loop {
            let text = self.str_of(fmt).unwrap_or("");
            let Some(k) = text[pos..].find(['{', '}']) else {
                if pos < text.len() {
                    let rest = self.substring(fmt, pos, text.len())?;
                    self.push(rest)?;
                }
                return Ok(());
            };
            let at = pos + k;
            let bytes = text.as_bytes();
            let brace = bytes[at];
            if bytes.get(at + 1) == Some(&brace) {
                // `{{` / `}}`: a literal brace
                let lit = self.substring(fmt, pos, at + 1)?;
                self.push(lit)?;
                pos = at + 2;
                continue;
            }
            if brace == b'}' {
                return Err(Error::ValueError); // single '}'
            }
            if at > pos {
                let lit = self.substring(fmt, pos, at)?;
                self.push(lit)?;
            }
            let text = self.str_of(fmt).unwrap_or("");
            let close = text[at..].find('}').ok_or(Error::ValueError)? + at;
            let field = &text[at + 1..close];
            let (name, rest) = match field.find(['!', ':']) {
                Some(i) => (&field[..i], &field[i..]),
                None => (field, ""),
            };
            let (conv, spec_text) = match rest.as_bytes().first() {
                Some(b'!') => {
                    let conv = rest.as_bytes().get(1).copied().ok_or(Error::ValueError)? as char;
                    let after = &rest[2..];
                    (conv, after.strip_prefix(':').unwrap_or(after))
                }
                Some(b':') => ('\0', &rest[1..]),
                _ => ('\0', ""),
            };
            let spec = Spec::parse(spec_text)?;
            let value = if name.is_empty() {
                auto += 1;
                *args.get(auto - 1).ok_or(Error::IndexError)?
            } else if let Ok(i) = name.parse::<usize>() {
                *args.get(i).ok_or(Error::IndexError)?
            } else {
                kwargs
                    .iter()
                    .find(|(k, _)| self.str_of(*k) == Some(name))
                    .map(|&(_, v)| v)
                    .ok_or(Error::KeyError)?
            };
            pos = close + 1;
            let piece = self.format_one(value, conv, &spec, 0)?;
            self.push(piece)?;
        }
    }

    /// `hex(n)`, `bin(n)`, `oct(n)`: `0xff`, `0b101`, `0o17` (`-0x1` for -1).
    pub(super) fn int_text(&mut self, v: Value<'a>, ty: char) -> Result<Value<'a>, Error> {
        if v.as_int().is_none() {
            return Err(Error::TypeError);
        }
        let spec = Spec {
            alt: true,
            ty,
            ..Spec::default()
        };
        self.format_one(v, '\0', &spec, 0)
    }

    /// `format(value, spec)`.
    #[inline(never)]
    pub(super) fn format_builtin(
        &mut self,
        value: Value<'a>,
        spec: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        let text = self.str_of(spec).ok_or(Error::TypeError)?;
        let spec = Spec::parse(text)?;
        self.format_one(value, '\0', &spec, 0)
    }
}
