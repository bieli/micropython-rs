// ============================================================================
// BYTES: b'...' literals, heap bytes, operators, methods, and the core
// `binascii` module
// ============================================================================
//
// Like strings, a bytes value is either `Value::Bytes` (a literal without
// escapes, pointing into the script source) or `Value::HBytes` (created at
// runtime, on the heap). Indexing gives an `int`, as in Python.

use super::{Interpreter, Value};
use crate::heap::Kind;
use crate::{Error, HardwareBridge, HwFn, Module, Output};

/// `import binascii`: hex conversion, used by tools such as Thonny to move
/// file contents over the serial port.
pub(crate) const BINASCII: Module = Module {
    name: "binascii",
    doc: "Hex conversion of bytes",
    functions: &[
        HwFn::new("hexlify", "data", "bytes -> lowercase hex digits (bytes)"),
        HwFn::new(
            "unhexlify",
            "hexdigits",
            "hex digits (bytes or str) -> bytes",
        ),
    ],
    constants: &[],
};

const HEX: &[u8; 16] = b"0123456789abcdef";

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    pub(super) fn bytes_of(&self, v: Value<'a>) -> Option<&[u8]> {
        match v {
            Value::Bytes(b) => Some(b),
            Value::HBytes(r) => Some(self.heap.bytes(r)),
            _ => None,
        }
    }

    pub(super) fn is_bytes(v: Value<'a>) -> bool {
        matches!(v, Value::Bytes(_) | Value::HBytes(_))
    }

    /// A `b'...'` literal: zero-copy unless it contains escape sequences.
    pub(super) fn bytes_literal(&mut self, raw: &'a str) -> Result<Value<'a>, Error> {
        if !raw.is_ascii() {
            return Err(Error::InvalidSyntax); // like Python: bytes literals are ASCII
        }
        if !raw.contains('\\') {
            return Ok(Value::Bytes(raw.as_bytes()));
        }
        let mut len = 0;
        decode_bytes_escapes(raw, &mut |_| len += 1)?;
        let r = self.alloc_obj(Kind::Bytes, len)?;
        let dst = self.heap.bytes_mut(r);
        let mut at = 0;
        decode_bytes_escapes(raw, &mut |b| {
            dst[at] = b;
            at += 1;
        })?;
        Ok(Value::HBytes(r))
    }

    /// New heap bytes filled by `fill`.
    fn new_bytes(&mut self, len: usize, fill: impl FnOnce(&mut [u8])) -> Result<Value<'a>, Error> {
        let r = self.alloc_obj(Kind::Bytes, len)?;
        fill(self.heap.bytes_mut(r));
        Ok(Value::HBytes(r))
    }

    /// `b[i]`: the byte as an `int`.
    pub(super) fn bytes_index(
        &mut self,
        obj: Value<'a>,
        index: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        let i = index.as_int().ok_or(Error::TypeError)?;
        let data = self.bytes_of(obj).ok_or(Error::TypeError)?;
        let n = data.len() as i64;
        let i = if i < 0 { i + n } else { i };
        if i < 0 || i >= n {
            return Err(Error::IndexError);
        }
        Ok(Value::Int(data[i as usize] as i64))
    }

    /// `b[a:b]`, clamped like Python.
    pub(super) fn bytes_slice(
        &mut self,
        obj: Value<'a>,
        start: Option<i64>,
        end: Option<i64>,
    ) -> Result<Value<'a>, Error> {
        let n = self.bytes_of(obj).ok_or(Error::TypeError)?.len() as i64;
        let clamp = |i: i64| (if i < 0 { i + n } else { i }).clamp(0, n);
        let (a, b) = (
            start.map_or(0, clamp) as usize,
            end.map_or(n, clamp) as usize,
        );
        if a >= b {
            return Ok(Value::Bytes(b""));
        }
        match obj {
            Value::Bytes(data) => Ok(Value::Bytes(&data[a..b])),
            _ => {
                let r = self.alloc_obj(Kind::Bytes, b - a)?;
                self.copy_into(obj, a, b, r, 0);
                Ok(Value::HBytes(r))
            }
        }
    }

    /// `b.method(args)` for bytes.
    pub(super) fn bytes_method(
        &mut self,
        v: Value<'a>,
        name: &str,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        let data = self.bytes_of(v).ok_or(Error::TypeError)?;
        let needle = args.first().and_then(|&a| self.bytes_of(a));
        match (name, args.len()) {
            ("hex", 0) => {
                let len = data.len() * 2;
                let r = self.alloc_obj(Kind::Str, len)?;
                self.copy_hex(v, r);
                Ok(Value::HStr(r))
            }
            ("decode", 0 | 1) => {
                if core::str::from_utf8(data).is_err() {
                    return Err(Error::ValueError); // UnicodeDecodeError in Python
                }
                match v {
                    Value::Bytes(b) => Ok(Value::Str(core::str::from_utf8(b).unwrap_or(""))),
                    _ => {
                        let r = self.alloc_obj(Kind::Str, data.len())?;
                        self.copy_into(v, 0, usize::MAX, r, 0);
                        Ok(Value::HStr(r))
                    }
                }
            }
            ("startswith", 1) => Ok(Value::Bool(
                data.starts_with(needle.ok_or(Error::TypeError)?),
            )),
            ("endswith", 1) => Ok(Value::Bool(data.ends_with(needle.ok_or(Error::TypeError)?))),
            ("find", 1) => {
                let needle = needle.ok_or(Error::TypeError)?;
                Ok(Value::Int(
                    find_bytes(data, needle).map_or(-1, |i| i as i64),
                ))
            }
            _ => Err(Error::AttributeError),
        }
    }

    /// Writes the hex digits of bytes `v` into string/bytes object `dst`.
    fn copy_hex(&mut self, v: Value<'a>, dst: crate::heap::ObjRef) {
        match v {
            Value::Bytes(data) => hex_into(data, self.heap.bytes_mut(dst)),
            Value::HBytes(src) => {
                let (data, out) = self.heap.src_dst(src, dst);
                hex_into(data, out);
            }
            _ => {}
        }
    }

    /// `bytes()`, `bytes(n)`, `bytes(b)`, `bytes(s, 'utf-8')`.
    pub(super) fn bytes_builtin(&mut self, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        match args {
            [] => Ok(Value::Bytes(b"")),
            [v] if Self::is_bytes(*v) => Ok(*v),
            [n] if n.as_int().is_some() => {
                let n = n.as_int().unwrap_or(0);
                if n < 0 {
                    return Err(Error::ValueError);
                }
                if n as usize > self.heap.capacity() {
                    return Err(Error::OutOfMemory);
                }
                self.new_bytes(n as usize, |_| {})
            }
            [s, _encoding] if Self::is_str(*s) => self.encode(*s),
            // `bytes([0x00, 0xAF])`, `bytes(range(16))`: from integers 0-255
            [it] if self.obj_kind(*it).is_some() => {
                let n = self.container_len(*it).ok_or(Error::TypeError)?;
                let r = self.alloc_obj(Kind::Bytes, n)?;
                let mut state = 0;
                for i in 0..n {
                    let v = self.iter_next(*it, &mut state)?.ok_or(Error::ValueError)?;
                    let b = v.as_int().ok_or(Error::TypeError)?;
                    let b = u8::try_from(b).map_err(|_| Error::ValueError)?;
                    self.heap.bytes_mut(r)[i] = b;
                }
                Ok(Value::HBytes(r))
            }
            _ => Err(Error::TypeError),
        }
    }

    /// `s.encode()`: the UTF-8 bytes of a string (zero-copy for source strings).
    pub(super) fn encode(&mut self, s: Value<'a>) -> Result<Value<'a>, Error> {
        match s {
            Value::Str(text) => Ok(Value::Bytes(text.as_bytes())),
            Value::HStr(_) => {
                let len = self.str_of(s).map_or(0, str::len);
                let r = self.alloc_obj(Kind::Bytes, len)?;
                self.copy_into(s, 0, usize::MAX, r, 0);
                Ok(Value::HBytes(r))
            }
            _ => Err(Error::TypeError),
        }
    }

    /// Functions of the core modules (`binascii`).
    pub(super) fn core_module_call(
        &mut self,
        module: &str,
        name: &str,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        match (module, name, args) {
            ("os", _, _) => self.os_call(name, args),
            ("sys", "exit", _) => {
                let class = super::exceptions::class_by_name("SystemExit").unwrap_or(0);
                Err(self.raise_new(class, args.first().copied()))
            }
            ("binascii", "hexlify", [v]) => {
                let len = self.bytes_of(*v).ok_or(Error::TypeError)?.len() * 2;
                let r = self.alloc_obj(Kind::Bytes, len)?;
                self.copy_hex(*v, r);
                Ok(Value::HBytes(r))
            }
            ("binascii", "unhexlify", [v]) => {
                let digits: &[u8] = match self.bytes_of(*v) {
                    Some(b) => b,
                    None => self.str_of(*v).ok_or(Error::TypeError)?.as_bytes(),
                };
                if !digits.len().is_multiple_of(2) || !digits.iter().all(u8::is_ascii_hexdigit) {
                    return Err(Error::ValueError); // binascii.Error in Python
                }
                let len = digits.len() / 2;
                let r = self.alloc_obj(Kind::Bytes, len)?;
                // Re-read the digits after the allocation (the heap may have moved)
                let v = *v;
                for i in 0..len {
                    let pair = match self.bytes_of(v) {
                        Some(b) => [b[2 * i], b[2 * i + 1]],
                        None => {
                            let s = self.str_of(v).unwrap_or("").as_bytes();
                            [s[2 * i], s[2 * i + 1]]
                        }
                    };
                    self.heap.bytes_mut(r)[i] = hex_value(pair[0]) << 4 | hex_value(pair[1]);
                }
                Ok(Value::HBytes(r))
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }
}

fn hex_into(data: &[u8], out: &mut [u8]) {
    for (i, b) in data.iter().enumerate() {
        out[2 * i] = HEX[(b >> 4) as usize];
        out[2 * i + 1] = HEX[(b & 0xF) as usize];
    }
}

fn hex_value(c: u8) -> u8 {
    (c as char).to_digit(16).unwrap_or(0) as u8
}

pub(crate) fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Decodes the escape sequences of a bytes literal: `\xNN`, octal, `\n`...
/// (`\u` isn't an escape in bytes, so it stays as written, like in Python).
fn decode_bytes_escapes(raw: &str, sink: &mut dyn FnMut(u8)) -> Result<(), Error> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        i += 1;
        if c != b'\\' || i == bytes.len() {
            sink(c);
            continue;
        }
        let e = bytes[i];
        i += 1;
        let simple = match e {
            b'\\' => Some(b'\\'),
            b'\'' => Some(b'\''),
            b'"' => Some(b'"'),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0C),
            b'v' => Some(0x0B),
            _ => None,
        };
        match (simple, e) {
            (Some(b), _) => sink(b),
            (None, b'x') => {
                let hi = bytes.get(i).and_then(|&c| (c as char).to_digit(16));
                let lo = bytes.get(i + 1).and_then(|&c| (c as char).to_digit(16));
                match (hi, lo) {
                    (Some(h), Some(l)) => sink((h * 16 + l) as u8),
                    _ => return Err(Error::InvalidSyntax),
                }
                i += 2;
            }
            (None, b'0'..=b'7') => {
                let mut code = (e - b'0') as u32;
                for _ in 0..2 {
                    match bytes.get(i) {
                        Some(&d @ b'0'..=b'7') => {
                            code = code * 8 + (d - b'0') as u32;
                            i += 1;
                        }
                        _ => break,
                    }
                }
                if code > 0xFF {
                    return Err(Error::InvalidSyntax);
                }
                sink(code as u8);
            }
            (None, other) => {
                sink(b'\\');
                sink(other);
            }
        }
    }
    Ok(())
}

/// Writes bytes as a Python literal: `b'...'` with CPython's escapes.
pub(crate) fn write_bytes_repr(data: &[u8], sink: &mut dyn FnMut(&str)) {
    let quote = if data.contains(&b'\'') && !data.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    sink("b");
    sink(if quote == b'"' { "\"" } else { "'" });
    for &b in data {
        match b {
            b'\\' => sink("\\\\"),
            b'\n' => sink("\\n"),
            b'\r' => sink("\\r"),
            b'\t' => sink("\\t"),
            b if b == quote => sink(if quote == b'\'' { "\\'" } else { "\\\"" }),
            0x20..=0x7E => {
                let one = [b];
                sink(core::str::from_utf8(&one).unwrap_or("?"));
            }
            _ => {
                let esc = [b'\\', b'x', HEX[(b >> 4) as usize], HEX[(b & 0xF) as usize]];
                sink(core::str::from_utf8(&esc).unwrap_or("?"));
            }
        }
    }
    sink(if quote == b'"' { "\"" } else { "'" });
}
