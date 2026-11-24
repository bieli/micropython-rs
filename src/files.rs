// ============================================================================
// FILES: the `os` module and `open()` on a `FileSystem` (e.g. the flash
// file system of the board, `flatfs`)
// ============================================================================
//
// The application gives the interpreter a file system with
// `Interpreter::with_files`. The file system is flat (no directories), like
// Thonny's "device without directories" mode: `os.listdir()` lists all files.
//
// A file object is a heap object laid out like a tuple of its name, its mode
// bits, the read position and the write token. Reads go straight from flash
// into the new string or bytes object; writes stream to flash, and the new
// version replaces the old one when the file is closed. One file can be open
// for writing at a time: opening another one saves the first.

use super::{Interpreter, Value};
use crate::flatfs::{FsError, MAX_NAME_LEN, SLOT_SIZE};
use crate::heap::{Kind, ObjRef};
use crate::{Error, HardwareBridge, HwFn, Module, Output};

/// `import os`
pub(crate) const OS: Module = Module {
    name: "os",
    doc: "Files on the board (a flat file system: no directories)",
    functions: &[
        HwFn::with_args("listdir", 0, 1, "[path]", "Names of all files"),
        HwFn::with_args(
            "ilistdir",
            0,
            1,
            "[path]",
            "(name, type, inode, size) of all files",
        ),
        HwFn::new("remove", "name", "Delete a file"),
        HwFn::new("rename", "old, new", "Rename a file"),
        HwFn::new("stat", "name", "(mode, 0, 0, 0, 0, 0, size, 0, 0, 0)"),
        HwFn::new("size", "name", "Size of a file in bytes"),
        HwFn::with_args(
            "statvfs",
            0,
            1,
            "[path]",
            "Space: (bsize, frsize, blocks, bfree, ...)",
        ),
        HwFn::new(
            "uname",
            "",
            "(sysname, nodename, release, version, machine)",
        ),
        HwFn::new("sync", "", "Nothing to do: files are saved when closed"),
    ],
    constants: &[],
};

const READ: i64 = 1;
const WRITE: i64 = 2;
const BINARY: i64 = 4;
const CLOSED: i64 = 8;

/// Slots of a file object.
const F_NAME: usize = 0;
const F_MODE: usize = 1;
const F_POS: usize = 2;
const F_TOKEN: usize = 3;

/// `os.stat()` mode bits of a file and of the (only) directory.
const S_IFREG: i64 = 0o100000;
const S_IFDIR: i64 = 0o040000;

/// A file name copied out of the heap, so the heap can change while it's used.
struct Name {
    buf: [u8; MAX_NAME_LEN],
    len: usize,
}

impl Name {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

/// The text of an `OSError`: the errno name first, as MicroPython prints it
/// (`[Errno 2] ENOENT`, which tools such as mpremote look for), then words.
fn message(err: FsError) -> &'static str {
    match err {
        FsError::NotFound => "ENOENT: no such file",
        FsError::Io => "EIO: flash I/O error",
        FsError::Corrupt => "EIO: file corrupted (checksum mismatch)",
        FsError::NotOpen => "EBADF: file not open for writing",
        FsError::NoRegion => "ENODEV: no file system",
        FsError::BadName => "EINVAL: bad file name (use A-Z a-z 0-9 . _ -, max 40 chars)",
        FsError::TooLarge => "EFBIG: file too large (max 32704 bytes)",
        FsError::NoSpace => "ENOSPC: no free file slot (saving needs one, even to replace)",
    }
}

/// The `OSError` errno for a file system error.
fn errno(err: FsError) -> i64 {
    match err {
        FsError::NotFound => 2,              // ENOENT
        FsError::Io | FsError::Corrupt => 5, // EIO
        FsError::NotOpen => 9,               // EBADF
        FsError::NoRegion => 19,             // ENODEV
        FsError::BadName => 22,              // EINVAL
        FsError::TooLarge => 27,             // EFBIG
        FsError::NoSpace => 28,              // ENOSPC
    }
}

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    /// Raises `OSError(errno, message)` for a file system error.
    fn os_error(&mut self, err: FsError) -> Error {
        let class = super::exceptions::class_by_name("OSError").unwrap_or(0);
        let args = [Value::Int(errno(err)), Value::Str(message(err))];
        match self.new_exception(class, &args) {
            Ok(exc) => {
                self.exc = exc;
                Error::Exception
            }
            Err(e) => e,
        }
    }

    fn fs<T>(
        &mut self,
        f: impl FnOnce(&mut dyn crate::flatfs::FileSystem) -> Result<T, FsError>,
    ) -> Result<T, Error> {
        let Some(files) = self.files else {
            return Err(self.os_error(FsError::NoRegion));
        };
        let result = f(&mut *files.borrow_mut());
        result.map_err(|e| self.os_error(e))
    }

    fn name_arg(&mut self, v: Value<'a>) -> Result<Name, Error> {
        let text = self.str_of(v).ok_or(Error::TypeError)?;
        // `/main.py` and `./main.py` are `main.py` (there are no directories)
        let text = text.trim_start_matches("./").trim_start_matches('/');
        if text.len() > MAX_NAME_LEN {
            return Err(self.os_error(FsError::BadName));
        }
        let mut name = Name {
            buf: [0; MAX_NAME_LEN],
            len: text.len(),
        };
        name.buf[..text.len()].copy_from_slice(text.as_bytes());
        Ok(name)
    }

    /// True for the paths that mean "the (only) directory".
    fn is_root(&self, v: Value<'a>) -> bool {
        self.str_of(v)
            .is_some_and(|p| matches!(p, "" | "/" | "." | "./"))
    }

    /// `os.function(args)`.
    pub(super) fn os_call(&mut self, name: &str, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        match (name, args) {
            ("listdir" | "ilistdir", []) | ("listdir" | "ilistdir", [_]) => {
                if let Some(&path) = args.first() {
                    if !self.is_root(path) {
                        return Err(self.os_error(FsError::NotFound));
                    }
                }
                let names = self.listdir()?;
                if name == "listdir" {
                    return Ok(names);
                }
                // `ilistdir`: `(name, type, inode, size)` per file (a list, not an iterator)
                let base = self.sp;
                self.push(names)?;
                let result = (|| {
                    let Value::Obj(list) = names else {
                        return Err(Error::TypeError);
                    };
                    for i in 0..self.count(list) {
                        let file = self.slot(list, i);
                        let n = self.name_arg(file)?;
                        let size = self.fs(|fs| fs.file_size(n.as_str()))?.unwrap_or(0);
                        let entry = self.tuple_of(&[
                            file,
                            Value::Int(S_IFREG),
                            Value::Int(0),
                            Value::Int(size as i64),
                        ])?;
                        self.set_slot(list, i, entry);
                    }
                    Ok(names)
                })();
                self.sp = base;
                result
            }
            ("remove", [n]) => {
                let n = self.name_arg(*n)?;
                self.fs(|fs| fs.remove(n.as_str())).map(|()| Value::None)
            }
            ("rename", [a, b]) => {
                let (a, b) = (self.name_arg(*a)?, self.name_arg(*b)?);
                self.fs(|fs| fs.rename(a.as_str(), b.as_str()))
                    .map(|()| Value::None)
            }
            ("stat", [n]) => {
                let (mode, size) = if self.is_root(*n) {
                    (S_IFDIR, 0)
                } else {
                    let n = self.name_arg(*n)?;
                    let size = self.fs(|fs| fs.file_size(n.as_str()))?;
                    let size = size.ok_or_else(|| self.os_error(FsError::NotFound))?;
                    (S_IFREG, size as i64)
                };
                let fields = [mode, 0, 0, 0, 0, 0, size, 0, 0, 0].map(Value::Int);
                self.tuple_of(&fields)
            }
            ("size", [n]) => {
                let n = self.name_arg(*n)?;
                let size = self.fs(|fs| fs.file_size(n.as_str()))?;
                size.map(|s| Value::Int(s as i64))
                    .ok_or_else(|| self.os_error(FsError::NotFound))
            }
            ("statvfs", _) => {
                // One block per slot: a file always takes a whole slot
                let (used, total) = self.fs(|fs| fs.usage())?;
                let free = (total - used) as i64;
                let block = SLOT_SIZE as i64;
                let fields = [
                    block,
                    block,
                    total as i64,
                    free,
                    free,
                    0,
                    0,
                    0,
                    0,
                    MAX_NAME_LEN as i64,
                ]
                .map(Value::Int);
                self.tuple_of(&fields)
            }
            ("uname", []) => {
                let release = env!("CARGO_PKG_VERSION");
                let fields = [
                    Value::Str("micropython-rs"),
                    Value::Str("micropython-rs"),
                    Value::Str(release),
                    Value::Str(concat!("micropython-rs ", env!("CARGO_PKG_VERSION"))),
                    Value::Str("micropython-rs board"),
                ];
                self.tuple_of(&fields)
            }
            ("sync", []) => Ok(Value::None),
            _ => Err(Error::TypeError),
        }
    }

    /// A new tuple of `values` (which need no rooting: ints and source strings).
    fn tuple_of(&mut self, values: &[Value<'a>]) -> Result<Value<'a>, Error> {
        let base = self.sp;
        let result = (|| {
            for &v in values {
                self.push(v)?;
            }
            self.build(Kind::Tuple, values.len())?;
            Ok(self.stack[self.sp - 1])
        })();
        self.sp = base;
        result
    }

    fn listdir(&mut self) -> Result<Value<'a>, Error> {
        // Collect the names first (the file system can't be borrowed while
        // the heap grows), at most one slot's worth per call of `list`
        let list = self.new_container(Kind::List, 0)?;
        let base = self.sp;
        self.push(Value::Obj(list))?;
        let result = (|| {
            let mut index = 0;
            loop {
                let mut name = Name {
                    buf: [0; MAX_NAME_LEN],
                    len: 0,
                };
                let mut seen = 0;
                self.fs(|fs| {
                    fs.list(&mut |n, _| {
                        if seen == index {
                            name.len = n.len().min(MAX_NAME_LEN);
                            name.buf[..name.len].copy_from_slice(&n.as_bytes()[..name.len]);
                        }
                        seen += 1;
                    })
                })?;
                if index >= seen {
                    return Ok(Value::Obj(list));
                }
                let r = self.alloc_obj(Kind::Str, name.len)?;
                self.heap
                    .bytes_mut(r)
                    .copy_from_slice(&name.buf[..name.len]);
                self.list_append(list, Value::HStr(r))?;
                index += 1;
            }
        })();
        self.sp = base;
        result
    }

    // ---- open() and file objects -------------------------------------------

    /// `open(name, [mode])` (`name` and `mode` rooted).
    pub(super) fn open(&mut self, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        let (name_v, mode) = match args {
            [n] => (*n, "r"),
            [n, m] => (*n, self.str_of(*m).ok_or(Error::TypeError)?),
            _ => return Err(Error::TypeError),
        };
        let mut bits = 0;
        for c in mode.chars() {
            bits |= match c {
                'r' => READ,
                'w' | 'a' => WRITE,
                'b' => BINARY,
                't' => 0,
                _ => return Err(Error::ValueError), // invalid mode
            };
        }
        let append = mode.contains('a');
        if bits & (READ | WRITE) == READ | WRITE || bits & (READ | WRITE) == 0 {
            return Err(Error::ValueError);
        }
        let name = self.name_arg(name_v)?;
        let token = if bits & WRITE != 0 {
            self.fs(|fs| fs.create(name.as_str(), append))?
        } else {
            match self.fs(|fs| fs.file_size(name.as_str()))? {
                Some(_) => 0,
                None => return Err(self.os_error(FsError::NotFound)),
            }
        };
        let stored_name = self.alloc_obj(Kind::Str, name.len)?;
        self.heap
            .bytes_mut(stored_name)
            .copy_from_slice(&name.buf[..name.len]);
        let base = self.sp;
        let result = (|| {
            self.push(Value::HStr(stored_name))?;
            for v in [bits, 0, token as i64] {
                self.push(Value::Int(v))?;
            }
            self.build(Kind::Tuple, 4)?;
            let file = self.stack[self.sp - 1];
            if let Value::Obj(r) = file {
                self.heap.set_kind(r, Kind::File);
            }
            Ok(file)
        })();
        self.sp = base;
        result
    }

    fn file_int(&self, r: ObjRef, slot: usize) -> i64 {
        match self.slot(r, slot) {
            Value::Int(n) => n,
            _ => 0,
        }
    }

    fn file_name(&mut self, r: ObjRef) -> Result<Name, Error> {
        self.name_arg(self.slot(r, F_NAME))
    }

    /// The mode bits of an open file, or `ValueError` if it's closed.
    fn open_mode(&self, r: ObjRef) -> Result<i64, Error> {
        let mode = self.file_int(r, F_MODE);
        if mode & CLOSED != 0 {
            return Err(Error::ValueError); // I/O operation on closed file
        }
        Ok(mode)
    }

    /// Reads up to `limit` bytes (`None`: to the end) at the read position
    /// into a new str/bytes object, and moves the position.
    fn file_read(
        &mut self,
        r: ObjRef,
        limit: Option<usize>,
        line: bool,
    ) -> Result<Value<'a>, Error> {
        let mode = self.open_mode(r)?;
        if mode & READ == 0 {
            return Err(self.os_error(FsError::NotOpen));
        }
        let name = self.file_name(r)?;
        let pos = self.file_int(r, F_POS) as u32;
        let size = self.fs(|fs| fs.file_size(name.as_str()))?.unwrap_or(0);
        let binary = mode & BINARY != 0;
        // In text mode `read(n)` counts characters: up to 4 bytes each
        let max_bytes = match (binary, limit) {
            (false, Some(n)) => n.saturating_mul(4),
            (_, Some(n)) => n,
            (_, None) => usize::MAX,
        };
        let mut len = (size.saturating_sub(pos) as usize).min(max_bytes);
        if line {
            // Find the end of the line, a chunk at a time
            let mut chunk = [0u8; 64];
            let mut scanned = 0;
            while scanned < len {
                let n = (len - scanned).min(chunk.len());
                let got =
                    self.fs(|fs| fs.read_at(name.as_str(), pos + scanned as u32, &mut chunk[..n]))?;
                if let Some(nl) = chunk[..got].iter().position(|&b| b == b'\n') {
                    len = scanned + nl + 1;
                    break;
                }
                if got == 0 {
                    len = scanned;
                    break;
                }
                scanned += got;
            }
        }
        let obj = self.alloc_obj(if binary { Kind::Bytes } else { Kind::Str }, len)?;
        let Some(files) = self.files else {
            return Err(self.os_error(FsError::NoRegion));
        };
        let got = files
            .borrow_mut()
            .read_at(name.as_str(), pos, self.heap.bytes_mut(obj))
            .map_err(|e| self.os_error(e))?;
        let mut used = got;
        if !binary {
            // Text: whole UTF-8 characters only (an incomplete one at the end
            // is read next time)
            let bytes = &self.heap.bytes(obj)[..got];
            let valid = match core::str::from_utf8(bytes) {
                Ok(_) => got,
                Err(e) if e.error_len().is_none() && got == len && limit.is_some() => {
                    e.valid_up_to()
                }
                Err(_) => return Err(Error::ValueError), // not valid UTF-8
            };
            let text = core::str::from_utf8(&bytes[..valid]).unwrap_or("");
            used = match limit {
                Some(n) if !line => text.char_indices().nth(n).map_or(valid, |(i, _)| i),
                _ => valid,
            };
        }
        if used != len {
            self.heap.realloc(obj, used);
        }
        self.set_file_int(r, F_POS, pos as i64 + used as i64);
        Ok(if binary {
            Value::HBytes(obj)
        } else {
            Value::HStr(obj)
        })
    }

    fn set_file_int(&mut self, r: ObjRef, slot: usize, v: i64) {
        self.set_slot(r, slot, Value::Int(v));
    }

    /// Methods of file objects; `None` if `receiver` isn't a file.
    pub(super) fn file_method(
        &mut self,
        receiver: Value<'a>,
        name: &str,
        args: &[Value<'a>],
    ) -> Option<Result<Value<'a>, Error>> {
        let Value::Obj(r) = receiver else {
            return None;
        };
        if self.heap.kind(r) != Kind::File {
            return None;
        }
        Some(self.file_call(r, name, args))
    }

    fn file_call(&mut self, r: ObjRef, name: &str, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        let limit = |v: Option<&Value<'a>>| -> Result<Option<usize>, Error> {
            match v {
                None | Some(Value::None) => Ok(None),
                Some(v) => {
                    let n = v.as_int().ok_or(Error::TypeError)?;
                    Ok(if n < 0 { None } else { Some(n as usize) })
                }
            }
        };
        match (name, args) {
            ("read", [] | [_]) => {
                let n = limit(args.first())?;
                self.file_read(r, n, false)
            }
            ("readline", []) => self.file_read(r, None, true),
            ("readlines", []) => {
                let list = self.new_container(Kind::List, 0)?;
                let base = self.sp;
                self.push(Value::Obj(list))?;
                let result = (|| loop {
                    let line = self.file_read(r, None, true)?;
                    if !self.truthy(line) {
                        return Ok(Value::Obj(list));
                    }
                    self.list_append(list, line)?;
                })();
                self.sp = base;
                result
            }
            ("write", [data]) => {
                let mode = self.open_mode(r)?;
                if mode & WRITE == 0 {
                    return Err(self.os_error(FsError::NotOpen));
                }
                let binary = mode & BINARY != 0;
                let (bytes, count) = match (binary, self.str_of(*data), self.bytes_of(*data)) {
                    (false, Some(s), _) => (s.as_bytes(), s.chars().count()),
                    (true, _, Some(b)) => (b, b.len()),
                    _ => return Err(Error::TypeError),
                };
                let token = self.file_int(r, F_TOKEN) as u32;
                let Some(files) = self.files else {
                    return Err(self.os_error(FsError::NoRegion));
                };
                let result = files.borrow_mut().write_file(token, bytes);
                result.map_err(|e| self.os_error(e))?;
                Ok(Value::Int(count as i64))
            }
            ("close", []) => {
                let mode = self.file_int(r, F_MODE);
                if mode & CLOSED == 0 {
                    self.set_file_int(r, F_MODE, mode | CLOSED);
                    if mode & WRITE != 0 {
                        let token = self.file_int(r, F_TOKEN) as u32;
                        self.fs(|fs| fs.close_file(token))?;
                    }
                }
                Ok(Value::None)
            }
            ("flush", []) => self.open_mode(r).map(|_| Value::None),
            ("tell", []) => Ok(Value::Int(self.file_int(r, F_POS))),
            ("seek", [pos] | [pos, _]) => {
                let whence = args.get(1).and_then(|w| w.as_int()).unwrap_or(0);
                let pos = pos.as_int().ok_or(Error::TypeError)?;
                let base = match whence {
                    0 => 0,
                    1 => self.file_int(r, F_POS),
                    _ => {
                        let name = self.file_name(r)?;
                        self.fs(|fs| fs.file_size(name.as_str()))?.unwrap_or(0) as i64
                    }
                };
                let new = (base + pos).max(0);
                self.set_file_int(r, F_POS, new);
                Ok(Value::Int(new))
            }
            _ => Err(Error::AttributeError),
        }
    }

    /// `f.name`, `f.closed`, `f.mode`.
    pub(super) fn file_attr(&self, v: Value<'a>, name: &str) -> Option<Value<'a>> {
        let Value::Obj(r) = v else { return None };
        if self.heap.kind(r) != Kind::File {
            return None;
        }
        let mode = self.file_int(r, F_MODE);
        match name {
            "name" => Some(self.slot(r, F_NAME)),
            "closed" => Some(Value::Bool(mode & CLOSED != 0)),
            "mode" => Some(Value::Str(match (mode & WRITE != 0, mode & BINARY != 0) {
                (false, false) => "r",
                (false, true) => "rb",
                (true, false) => "w",
                (true, true) => "wb",
            })),
            _ => None,
        }
    }

    /// The next line of a file in a `for` loop, or `None` at the end.
    pub(super) fn file_next_line(&mut self, r: ObjRef) -> Result<Option<Value<'a>>, Error> {
        let line = self.file_read(r, None, true)?;
        Ok(if self.truthy(line) { Some(line) } else { None })
    }

    /// Closes a file object (for `with`); other values are left alone.
    pub(super) fn close_if_file(&mut self, v: Value<'a>) -> Result<(), Error> {
        match v {
            Value::Obj(r) if self.heap.kind(r) == Kind::File => {
                self.file_call(r, "close", &[]).map(|_| ())
            }
            _ => Ok(()),
        }
    }
}
