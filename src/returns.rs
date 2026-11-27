// ============================================================================
// RETURNS: rich return values from Rust (floats, strings, tuples, lists, dicts)
// ============================================================================
//
// A hardware function marked `HwFn::returning_values()` gets a `Returns` to
// fill instead of returning an `i64` (`HardwareBridge::call_values`):
//
//     out.float(21.37)?;                     // one value: a float
//
//     out.begin_tuple()?;                    // (21.37, 'ok', b'\x01')
//     out.float(21.37)?;
//     out.str("ok")?;
//     out.bytes(&[1])?;
//     out.end()?;
//
// `Returns` only records the values, as compact typed records, in a scratch
// area at the top of the heap (the arguments' strings live below it, so they
// stay valid: nothing is allocated, nothing moves during the call). After the
// call the interpreter builds the Python objects from the records. Nothing
// recorded: `None`; several values at the top level: a tuple of them.
//
// Limits: the records must fit the scratch area (the heap's free memory, up
// to 64 KB: a string takes its length + 5 bytes, a number 9); containers nest
// up to `MAX_RETURN_DEPTH` deep; dict keys must be hashable (no lists).

use crate::Error;

/// How deep `begin_*` calls may nest.
pub const MAX_RETURN_DEPTH: usize = 8;

/// One value for `Returns::value`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ret<'r> {
    None,
    Bool(bool),
    Int(i64),
    /// In the build's float precision (`f32` with the `float32` feature).
    Float(f64),
    Str(&'r str),
    Bytes(&'r [u8]),
}

pub(crate) const TAG_NONE: u8 = 0;
pub(crate) const TAG_FALSE: u8 = 1;
pub(crate) const TAG_TRUE: u8 = 2;
pub(crate) const TAG_INT: u8 = 3;
pub(crate) const TAG_FLOAT: u8 = 4;
pub(crate) const TAG_STR: u8 = 5;
pub(crate) const TAG_BYTES: u8 = 6;
pub(crate) const TAG_TUPLE: u8 = 7;
pub(crate) const TAG_LIST: u8 = 8;
pub(crate) const TAG_DICT: u8 = 9;
pub(crate) const TAG_END: u8 = 10;

/// The values a `returning_values` function gives back (see the module docs).
pub struct Returns<'b> {
    buf: &'b mut [u8],
    len: usize,
    depth: usize,
}

impl<'b> Returns<'b> {
    /// Records into `buf` (the interpreter makes it; also for tests).
    pub fn new(buf: &'b mut [u8]) -> Self {
        Self {
            buf,
            len: 0,
            depth: 0,
        }
    }

    /// Bytes recorded so far.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Containers begun and not ended yet.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// The records (what the interpreter decodes).
    pub fn records(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let end = self.len + bytes.len();
        // The scratch area is full: a `MemoryError` in the script
        let dst = self.buf.get_mut(self.len..end).ok_or(Error::OutOfMemory)?;
        dst.copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }

    /// One value (a number, a string, ...).
    pub fn value(&mut self, v: Ret<'_>) -> Result<(), Error> {
        match v {
            Ret::None => self.put(&[TAG_NONE]),
            Ret::Bool(b) => self.put(&[if b { TAG_TRUE } else { TAG_FALSE }]),
            Ret::Int(n) => {
                self.put(&[TAG_INT])?;
                self.put(&n.to_le_bytes())
            }
            Ret::Float(f) => {
                self.put(&[TAG_FLOAT])?;
                self.put(&f.to_le_bytes())
            }
            Ret::Str(s) => self.blob(TAG_STR, s.as_bytes()),
            Ret::Bytes(b) => self.blob(TAG_BYTES, b),
        }
    }

    fn blob(&mut self, tag: u8, data: &[u8]) -> Result<(), Error> {
        let len = u32::try_from(data.len()).map_err(|_| Error::OutOfMemory)?;
        if self.buf.len() - self.len < 5 + data.len() {
            return Err(Error::OutOfMemory);
        }
        self.put(&[tag])?;
        self.put(&len.to_le_bytes())?;
        self.put(data)
    }

    pub fn none(&mut self) -> Result<(), Error> {
        self.value(Ret::None)
    }

    pub fn bool(&mut self, b: bool) -> Result<(), Error> {
        self.value(Ret::Bool(b))
    }

    pub fn int(&mut self, n: i64) -> Result<(), Error> {
        self.value(Ret::Int(n))
    }

    pub fn float(&mut self, f: f64) -> Result<(), Error> {
        self.value(Ret::Float(f))
    }

    pub fn str(&mut self, s: &str) -> Result<(), Error> {
        self.value(Ret::Str(s))
    }

    pub fn bytes(&mut self, b: &[u8]) -> Result<(), Error> {
        self.value(Ret::Bytes(b))
    }

    fn begin(&mut self, tag: u8) -> Result<(), Error> {
        if self.depth >= MAX_RETURN_DEPTH {
            return Err(Error::ValueError);
        }
        self.put(&[tag])?;
        self.depth += 1;
        Ok(())
    }

    /// Starts a tuple: the values until `end` are its items.
    pub fn begin_tuple(&mut self) -> Result<(), Error> {
        self.begin(TAG_TUPLE)
    }

    /// Starts a list (it may be long: it's built in chunks).
    pub fn begin_list(&mut self) -> Result<(), Error> {
        self.begin(TAG_LIST)
    }

    /// Starts a dict: the values until `end` alternate key, value.
    pub fn begin_dict(&mut self) -> Result<(), Error> {
        self.begin(TAG_DICT)
    }

    /// Ends the innermost tuple, list or dict.
    pub fn end(&mut self) -> Result<(), Error> {
        if self.depth == 0 {
            return Err(Error::ValueError);
        }
        self.put(&[TAG_END])?;
        self.depth -= 1;
        Ok(())
    }
}
