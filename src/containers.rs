// ============================================================================
// CONTAINERS: list, tuple, dict and range objects on the heap
// ============================================================================
//
// A container is a heap object (`Value::Obj`): a 4-byte length followed by
// encoded values ("slots", 9 bytes each: a tag and 8 bytes of payload). A
// list grows by reallocating its object, which keeps its handle, so every
// variable referring to the list sees the change. A dict stores key/value
// slot pairs in insertion order and finds keys by linear search: dicts on a
// microcontroller are small.
//
// Slots can't hold references into the script source, so a source string
// (`Value::Str`) is copied to the heap when it's stored in a container.
//
// The collector traces through containers: a reachable list keeps its
// elements alive (see `collect`).

use core::cmp::Ordering as CmpOrdering;

use super::{Interpreter, Value};
use crate::ast_parser::{BinOp, CmpOp};
use crate::heap::{HeapView, Kind, ObjRef, MAX_OBJECTS};
use crate::{Error, HardwareBridge, Output};

/// Bytes before the first slot: the number of elements (or pairs).
const HEADER: usize = 4;
/// Bytes per stored value.
const SLOT: usize = 9;
/// Nesting depth up to which containers are printed (deeper: `[...]`).
const MAX_RENDER_DEPTH: usize = 8;

const T_NONE: u8 = 0;
const T_FALSE: u8 = 1;
const T_TRUE: u8 = 2;
const T_INT: u8 = 3;
const T_HSTR: u8 = 4;
const T_HBYTES: u8 = 5;
const T_OBJ: u8 = 6;
const T_MODULE: u8 = 7;
const T_NATIVE: u8 = 8;
const T_EMPTY_STR: u8 = 9;
const T_EMPTY_BYTES: u8 = 10;
const T_EXC_CLASS: u8 = 11;

/// Encodes a value for a slot. Source strings must be copied to the heap
/// first (`Interpreter::stored`); only empty ones are encoded directly.
fn encode(v: Value<'_>) -> [u8; SLOT] {
    let mut slot = [0u8; SLOT];
    let (tag, payload): (u8, u64) = match v {
        Value::None => (T_NONE, 0),
        Value::Bool(false) => (T_FALSE, 0),
        Value::Bool(true) => (T_TRUE, 0),
        Value::Int(n) => (T_INT, n as u64),
        Value::HStr(r) => (T_HSTR, r.index() as u64),
        Value::HBytes(r) => (T_HBYTES, r.index() as u64),
        Value::Obj(r) => (T_OBJ, r.index() as u64),
        Value::Module(m) => (T_MODULE, m as u64),
        Value::NativeFn { module, func } => (T_NATIVE, (module as u64) << 8 | func as u64),
        Value::ExcClass(c) => (T_EXC_CLASS, c as u64),
        Value::Str(_) => (T_EMPTY_STR, 0),
        Value::Bytes(_) => (T_EMPTY_BYTES, 0),
    };
    slot[0] = tag;
    slot[1..].copy_from_slice(&payload.to_le_bytes());
    slot
}

fn decode<'a>(slot: &[u8]) -> Value<'a> {
    let mut payload = [0u8; 8];
    payload.copy_from_slice(&slot[1..SLOT]);
    let n = u64::from_le_bytes(payload);
    let r = || ObjRef::from_index(n as usize);
    match slot[0] {
        T_FALSE => Value::Bool(false),
        T_TRUE => Value::Bool(true),
        T_INT => Value::Int(n as i64),
        T_HSTR => Value::HStr(r()),
        T_HBYTES => Value::HBytes(r()),
        T_OBJ => Value::Obj(r()),
        T_MODULE => Value::Module(n as u8),
        T_NATIVE => Value::NativeFn {
            module: (n >> 8) as u8,
            func: n as u8,
        },
        T_EMPTY_STR => Value::Str(""),
        T_EMPTY_BYTES => Value::Bytes(b""),
        T_EXC_CLASS => Value::ExcClass(n as u8),
        _ => Value::None,
    }
}

/// Number of elements (pairs for a dict) of a container.
pub(super) fn count(view: &HeapView<'_>, r: ObjRef) -> usize {
    let b = view.bytes(r);
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
}

/// Slot `i` of a container (for a dict, slot `2k` is a key, `2k + 1` its value).
pub(super) fn slot<'a>(view: &HeapView<'_>, r: ObjRef, i: usize) -> Value<'a> {
    let at = HEADER + i * SLOT;
    decode(&view.bytes(r)[at..at + SLOT])
}

fn range_parts(view: &HeapView<'_>, r: ObjRef) -> (i64, i64, i64) {
    let b = view.bytes(r);
    let part = |i: usize| {
        let mut x = [0u8; 8];
        x.copy_from_slice(&b[i * 8..i * 8 + 8]);
        i64::from_le_bytes(x)
    };
    (part(0), part(1), part(2))
}

fn range_len((start, stop, step): (i64, i64, i64)) -> usize {
    let (span, step) = if step > 0 {
        (stop as i128 - start as i128, step as i128)
    } else {
        (start as i128 - stop as i128, -(step as i128))
    };
    if span <= 0 {
        0
    } else {
        ((span + step - 1) / step) as usize
    }
}

/// Python index rules: negative counts from the end; `None` if out of range.
fn norm_index(i: i64, len: usize) -> Option<usize> {
    let i = if i < 0 { i + len as i64 } else { i };
    (0..len as i64).contains(&i).then_some(i as usize)
}

/// Writes `v` as Python's `str()` (`repr == false`) or `repr()` would.
pub(crate) fn render(
    view: &HeapView<'_>,
    modules: super::strings::ModuleLookup<'_>,
    v: Value<'_>,
    repr: bool,
    depth: usize,
    sink: &mut dyn FnMut(&str),
) {
    match v {
        Value::HStr(r) => {
            if repr {
                super::strings::write_str_repr(view.str(r), sink)
            } else {
                sink(view.str(r))
            }
        }
        Value::HBytes(r) => super::bytes::write_bytes_repr(view.bytes(r), sink),
        Value::Obj(r) => {
            let kind = view.kind(r);
            if kind == Kind::Exception {
                // `ValueError('bad')` (repr) or `bad` (str)
                let class = match slot(view, r, 0) {
                    Value::Int(c) => c as u8,
                    _ => 0,
                };
                let n = count(view, r) - 1;
                let key_error = super::exceptions::class_name(class) == "KeyError";
                if repr {
                    sink(super::exceptions::class_name(class));
                    sink("(");
                } else if n == 1 {
                    let arg = slot(view, r, 1);
                    return render(view, modules, arg, key_error, depth + 1, sink);
                } else if n == 0 {
                    return;
                } else if let (true, 2, Value::Int(errno)) =
                    (super::exceptions::is_os_error(class), n, slot(view, r, 1))
                {
                    // `OSError(2, 'no file')` prints as `[Errno 2] no file`
                    let mut buf = [0u8; 20];
                    sink("[Errno ");
                    sink(super::strings::format_i64(errno, &mut buf));
                    sink("] ");
                    return render(view, modules, slot(view, r, 2), false, depth + 1, sink);
                } else {
                    sink("("); // several arguments: str() is the tuple of them
                }
                for i in 0..n {
                    if i > 0 {
                        sink(", ");
                    }
                    render(view, modules, slot(view, r, i + 1), true, depth + 1, sink);
                }
                sink(")");
                return;
            }
            if kind == Kind::Range {
                let (start, stop, step) = range_parts(view, r);
                let mut buf = [0u8; 20];
                sink("range(");
                sink(super::strings::format_i64(start, &mut buf));
                sink(", ");
                sink(super::strings::format_i64(stop, &mut buf));
                if step != 1 {
                    sink(", ");
                    sink(super::strings::format_i64(step, &mut buf));
                }
                sink(")");
                return;
            }
            let (open, close) = match kind {
                Kind::List => ("[", "]"),
                Kind::Tuple => ("(", ")"),
                _ => ("{", "}"),
            };
            sink(open);
            if depth >= MAX_RENDER_DEPTH {
                sink("...");
                sink(close);
                return;
            }
            let n = count(view, r);
            for i in 0..n {
                if i > 0 {
                    sink(", ");
                }
                if kind == Kind::Dict {
                    render(view, modules, slot(view, r, 2 * i), true, depth + 1, sink);
                    sink(": ");
                    render(
                        view,
                        modules,
                        slot(view, r, 2 * i + 1),
                        true,
                        depth + 1,
                        sink,
                    );
                } else {
                    render(view, modules, slot(view, r, i), true, depth + 1, sink);
                }
            }
            if kind == Kind::Tuple && n == 1 {
                sink(",");
            }
            sink(close);
        }
        other => super::strings::write_plain(other, modules, repr, sink),
    }
}

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    pub(super) fn obj_kind(&self, v: Value<'a>) -> Option<Kind> {
        match v {
            Value::Obj(r) => Some(self.heap.kind(r)),
            _ => None,
        }
    }

    /// `v` as a list/tuple handle, if it is one.
    fn seq(&self, v: Value<'a>) -> Option<ObjRef> {
        match v {
            Value::Obj(r) if matches!(self.heap.kind(r), Kind::List | Kind::Tuple) => Some(r),
            _ => None,
        }
    }

    fn dict(&self, v: Value<'a>) -> Option<ObjRef> {
        match v {
            Value::Obj(r) if self.heap.kind(r) == Kind::Dict => Some(r),
            _ => None,
        }
    }

    pub(super) fn count(&self, r: ObjRef) -> usize {
        count(&self.heap.view(), r)
    }

    pub(super) fn slot(&self, r: ObjRef, i: usize) -> Value<'a> {
        slot(&self.heap.view(), r, i)
    }

    fn set_count(&mut self, r: ObjRef, n: usize) {
        self.heap.bytes_mut(r)[..HEADER].copy_from_slice(&(n as u32).to_le_bytes());
    }

    /// Writes a value that is already in stored form (see `stored`).
    fn set_slot(&mut self, r: ObjRef, i: usize, v: Value<'a>) {
        let at = HEADER + i * SLOT;
        self.heap.bytes_mut(r)[at..at + SLOT].copy_from_slice(&encode(v));
    }

    fn slot_size(kind: Kind) -> usize {
        if kind == Kind::Dict {
            2 * SLOT
        } else {
            SLOT
        }
    }

    fn capacity(&self, r: ObjRef) -> usize {
        (self.heap.bytes(r).len() - HEADER) / Self::slot_size(self.heap.kind(r))
    }

    /// A value as it can be stored in a slot: non-empty source strings and
    /// bytes are copied to the heap (so the result must be rooted before the
    /// next allocation).
    pub(super) fn stored(&mut self, v: Value<'a>) -> Result<Value<'a>, Error> {
        match v {
            Value::Str(s) if !s.is_empty() => {
                let r = self.alloc_obj(Kind::Str, s.len())?;
                self.heap.bytes_mut(r).copy_from_slice(s.as_bytes());
                Ok(Value::HStr(r))
            }
            Value::Bytes(b) if !b.is_empty() => {
                let r = self.alloc_obj(Kind::Bytes, b.len())?;
                self.heap.bytes_mut(r).copy_from_slice(b);
                Ok(Value::HBytes(r))
            }
            other => Ok(other),
        }
    }

    /// A new empty list, tuple or dict with room for `cap` elements (pairs).
    pub(super) fn new_container(&mut self, kind: Kind, cap: usize) -> Result<ObjRef, Error> {
        let size = cap
            .checked_mul(Self::slot_size(kind))
            .and_then(|n| n.checked_add(HEADER))
            .filter(|&n| n <= self.heap.capacity())
            .ok_or(Error::OutOfMemory)?;
        self.alloc_obj(kind, size)
    }

    /// Makes room for `need` elements (`r` must be rooted).
    fn reserve(&mut self, r: ObjRef, need: usize) -> Result<(), Error> {
        let cap = self.capacity(r);
        if need <= cap {
            return Ok(());
        }
        let new_cap = need.max(cap * 2).max(4);
        let size = HEADER + new_cap * Self::slot_size(self.heap.kind(r));
        if self.heap.realloc(r, size) {
            return Ok(());
        }
        self.collect();
        if self.heap.realloc(r, size) {
            return Ok(());
        }
        // Try again with exactly what's needed
        let size = HEADER + need * Self::slot_size(self.heap.kind(r));
        if self.heap.realloc(r, size) {
            Ok(())
        } else {
            Err(Error::OutOfMemory)
        }
    }

    /// `list.append(v)`; `r` and `v` must be rooted.
    pub(super) fn list_append(&mut self, r: ObjRef, v: Value<'a>) -> Result<(), Error> {
        let v = self.stored(v)?;
        let base = self.sp;
        self.push(v)?;
        let n = self.count(r);
        let result = self.reserve(r, n + 1);
        if result.is_ok() {
            self.set_slot(r, n, v);
            self.set_count(r, n + 1);
        }
        self.sp = base;
        result
    }

    /// A new list or tuple of the values `self.stack[from..from + n]`.
    fn build_seq(&mut self, kind: Kind, from: usize, n: usize) -> Result<Value<'a>, Error> {
        let r = self.new_container(kind, n)?;
        self.push(Value::Obj(r))?;
        for k in 0..n {
            let v = self.stored(self.stack[from + k])?;
            self.set_slot(r, k, v);
            self.set_count(r, k + 1);
        }
        self.sp -= 1;
        Ok(Value::Obj(r))
    }

    /// `BuildList` / `BuildTuple` / `BuildDict`: replaces the top `n` values
    /// (`2n` for a dict) with the new container.
    pub(super) fn build(&mut self, kind: Kind, n: usize) -> Result<(), Error> {
        let values = if kind == Kind::Dict { 2 * n } else { n };
        let from = self.sp - values;
        let val = if kind == Kind::Dict {
            let r = self.new_container(Kind::Dict, n)?;
            self.push(Value::Obj(r))?;
            for k in 0..n {
                let (key, v) = (self.stack[from + 2 * k], self.stack[from + 2 * k + 1]);
                self.dict_set(r, key, v)?;
            }
            self.sp -= 1;
            Value::Obj(r)
        } else {
            self.build_seq(kind, from, n)?
        };
        self.sp = from;
        self.push(val)
    }

    // ---- dicts -------------------------------------------------------

    fn check_hashable(&self, key: Value<'a>) -> Result<(), Error> {
        match self.obj_kind(key) {
            Some(Kind::List | Kind::Dict) => Err(Error::TypeError),
            Some(Kind::Tuple) => {
                let r = self.seq(key).ok_or(Error::TypeError)?;
                (0..self.count(r)).try_for_each(|i| self.check_hashable(self.slot(r, i)))
            }
            _ => Ok(()),
        }
    }

    /// Index of the pair with `key`.
    pub(super) fn dict_find(&self, r: ObjRef, key: Value<'a>) -> Result<Option<usize>, Error> {
        self.check_hashable(key)?;
        Ok((0..self.count(r)).find(|&i| self.equal(self.slot(r, 2 * i), key)))
    }

    /// `d[key] = v`; `r`, `key` and `v` must be rooted.
    pub(super) fn dict_set(
        &mut self,
        r: ObjRef,
        key: Value<'a>,
        v: Value<'a>,
    ) -> Result<(), Error> {
        let found = self.dict_find(r, key)?;
        let base = self.sp;
        let result = (|| {
            let v = self.stored(v)?;
            self.push(v)?;
            match found {
                Some(i) => self.set_slot(r, 2 * i + 1, v),
                None => {
                    let k = self.stored(key)?;
                    self.push(k)?;
                    let n = self.count(r);
                    self.reserve(r, n + 1)?;
                    self.set_slot(r, 2 * n, k);
                    self.set_slot(r, 2 * n + 1, v);
                    self.set_count(r, n + 1);
                }
            }
            Ok(())
        })();
        self.sp = base;
        result
    }

    fn dict_remove(&mut self, r: ObjRef, i: usize) {
        let n = self.count(r);
        let from = HEADER + (i + 1) * 2 * SLOT;
        let to = HEADER + n * 2 * SLOT;
        self.heap
            .bytes_mut(r)
            .copy_within(from..to, from - 2 * SLOT);
        self.set_count(r, n - 1);
    }

    // ---- equality, ordering, membership --------------------------------

    /// `a == b` with Python semantics (containers compare element-wise).
    pub(super) fn equal(&self, a: Value<'a>, b: Value<'a>) -> bool {
        self.equal_at(a, b, 0)
    }

    fn equal_at(&self, a: Value<'a>, b: Value<'a>, depth: usize) -> bool {
        if let (Some(x), Some(y)) = (self.str_of(a), self.str_of(b)) {
            return x == y;
        }
        if let (Some(x), Some(y)) = (self.bytes_of(a), self.bytes_of(b)) {
            return x == y;
        }
        if let (Some(x), Some(y)) = (a.as_int(), b.as_int()) {
            return x == y;
        }
        match (a, b) {
            (Value::Obj(x), Value::Obj(y)) => {
                if x == y {
                    return true;
                }
                let (kx, ky) = (self.heap.kind(x), self.heap.kind(y));
                if kx != ky || depth >= MAX_RENDER_DEPTH {
                    return false;
                }
                let view = self.heap.view();
                match kx {
                    Kind::Range => range_parts(&view, x) == range_parts(&view, y),
                    Kind::Dict => {
                        let n = self.count(x);
                        n == self.count(y)
                            && (0..n).all(|i| {
                                let key = self.slot(x, 2 * i);
                                (0..n).any(|j| {
                                    self.equal_at(key, self.slot(y, 2 * j), depth + 1)
                                        && self.equal_at(
                                            self.slot(x, 2 * i + 1),
                                            self.slot(y, 2 * j + 1),
                                            depth + 1,
                                        )
                                })
                            })
                    }
                    _ => {
                        let n = self.count(x);
                        n == self.count(y)
                            && (0..n)
                                .all(|i| self.equal_at(self.slot(x, i), self.slot(y, i), depth + 1))
                    }
                }
            }
            (Value::None, Value::None) => true,
            (Value::Module(_) | Value::NativeFn { .. } | Value::ExcClass(_), _) => a == b,
            _ => false,
        }
    }

    /// `a is b`: the same object (equal values for numbers, `None` and bools).
    pub(super) fn identical(&self, a: Value<'a>, b: Value<'a>) -> bool {
        match (a, b) {
            (Value::Str(x), Value::Str(y)) => x.as_ptr() == y.as_ptr() && x.len() == y.len(),
            (Value::Bytes(x), Value::Bytes(y)) => x.as_ptr() == y.as_ptr() && x.len() == y.len(),
            _ => a == b,
        }
    }

    /// Ordering of two lists or two tuples (element by element).
    pub(super) fn seq_order(&self, a: Value<'a>, b: Value<'a>) -> Result<CmpOrdering, Error> {
        let (Some(x), Some(y)) = (self.seq(a), self.seq(b)) else {
            return Err(Error::TypeError);
        };
        if self.heap.kind(x) != self.heap.kind(y) {
            return Err(Error::TypeError);
        }
        let (n, m) = (self.count(x), self.count(y));
        for i in 0..n.min(m) {
            let (p, q) = (self.slot(x, i), self.slot(y, i));
            if !self.equal(p, q) {
                return Ok(if self.compare(CmpOp::Lt, p, q)? {
                    CmpOrdering::Less
                } else {
                    CmpOrdering::Greater
                });
            }
        }
        Ok(n.cmp(&m))
    }

    /// `x in container`, or `None` if `container` isn't a container.
    pub(super) fn contains(
        &self,
        container: Value<'a>,
        x: Value<'a>,
    ) -> Option<Result<bool, Error>> {
        let r = match container {
            Value::Obj(r) => r,
            _ => return None,
        };
        Some(match self.heap.kind(r) {
            Kind::List | Kind::Tuple => {
                Ok((0..self.count(r)).any(|i| self.equal(self.slot(r, i), x)))
            }
            Kind::Dict => self.dict_find(r, x).map(|i| i.is_some()),
            Kind::Range => {
                let (start, _, step) = range_parts(&self.heap.view(), r);
                let len = range_len(range_parts(&self.heap.view(), r));
                Ok(x.as_int().is_some_and(|n| {
                    let off = n as i128 - start as i128;
                    off % step as i128 == 0 && (0..len as i128).contains(&(off / step as i128))
                }))
            }
            _ => Err(Error::TypeError),
        })
    }

    // ---- len, indexing, slicing, assignment ----------------------------

    /// `len(v)` of a container.
    pub(super) fn container_len(&self, v: Value<'a>) -> Option<usize> {
        let Value::Obj(r) = v else {
            return None;
        };
        Some(match self.heap.kind(r) {
            Kind::Range => range_len(range_parts(&self.heap.view(), r)),
            _ => self.count(r),
        })
    }

    /// `container[index]`.
    pub(super) fn container_index(
        &mut self,
        r: ObjRef,
        index: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        match self.heap.kind(r) {
            Kind::Dict => {
                let i = self.dict_find(r, index)?.ok_or(Error::KeyError)?;
                Ok(self.slot(r, 2 * i + 1))
            }
            Kind::Range => {
                let parts = range_parts(&self.heap.view(), r);
                let i = index.as_int().ok_or(Error::TypeError)?;
                let i = norm_index(i, range_len(parts)).ok_or(Error::IndexError)?;
                Ok(Value::Int(parts.0 + i as i64 * parts.2))
            }
            _ => {
                let i = index.as_int().ok_or(Error::TypeError)?;
                let i = norm_index(i, self.count(r)).ok_or(Error::IndexError)?;
                Ok(self.slot(r, i))
            }
        }
    }

    /// `seq[a:b]` of a list or tuple: a new list / tuple.
    pub(super) fn container_slice(
        &mut self,
        r: ObjRef,
        start: Option<i64>,
        end: Option<i64>,
    ) -> Result<Value<'a>, Error> {
        let kind = self.heap.kind(r);
        if !matches!(kind, Kind::List | Kind::Tuple) {
            return Err(Error::TypeError);
        }
        let n = self.count(r) as i64;
        let clamp = |i: i64| (if i < 0 { i + n } else { i }).clamp(0, n);
        let (a, b) = (
            start.map_or(0, clamp) as usize,
            end.map_or(n, clamp) as usize,
        );
        let len = b.saturating_sub(a);
        let dst = self.new_container(kind, len)?;
        for k in 0..len {
            let v = self.slot(r, a + k);
            self.set_slot(dst, k, v);
        }
        self.set_count(dst, len);
        Ok(Value::Obj(dst))
    }

    /// `container[key] = v` (all three rooted).
    pub(super) fn set_item(
        &mut self,
        container: Value<'a>,
        key: Value<'a>,
        v: Value<'a>,
    ) -> Result<(), Error> {
        let Value::Obj(r) = container else {
            return Err(Error::TypeError);
        };
        match self.heap.kind(r) {
            Kind::List => {
                let i = key.as_int().ok_or(Error::TypeError)?;
                let i = norm_index(i, self.count(r)).ok_or(Error::IndexError)?;
                let v = self.stored(v)?;
                self.set_slot(r, i, v);
                Ok(())
            }
            Kind::Dict => self.dict_set(r, key, v),
            _ => Err(Error::TypeError),
        }
    }

    /// `del container[key]`.
    pub(super) fn del_item(&mut self, container: Value<'a>, key: Value<'a>) -> Result<(), Error> {
        let Value::Obj(r) = container else {
            return Err(Error::TypeError);
        };
        match self.heap.kind(r) {
            Kind::List => {
                let i = key.as_int().ok_or(Error::TypeError)?;
                let i = norm_index(i, self.count(r)).ok_or(Error::IndexError)?;
                self.list_remove_at(r, i);
                Ok(())
            }
            Kind::Dict => {
                let i = self.dict_find(r, key)?.ok_or(Error::KeyError)?;
                self.dict_remove(r, i);
                Ok(())
            }
            _ => Err(Error::TypeError),
        }
    }

    fn list_remove_at(&mut self, r: ObjRef, i: usize) {
        let n = self.count(r);
        let from = HEADER + (i + 1) * SLOT;
        self.heap
            .bytes_mut(r)
            .copy_within(from..HEADER + n * SLOT, from - SLOT);
        self.set_count(r, n - 1);
    }

    // ---- operators -----------------------------------------------------

    /// `+` and `*` on lists and tuples.
    pub(super) fn container_binop(
        &mut self,
        op: BinOp,
        lhs: Value<'a>,
        rhs: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        match op {
            BinOp::Add => {
                let (Some(x), Some(y)) = (self.seq(lhs), self.seq(rhs)) else {
                    return Err(Error::TypeError);
                };
                let kind = self.heap.kind(x);
                if kind != self.heap.kind(y) {
                    return Err(Error::TypeError);
                }
                let (n, m) = (self.count(x), self.count(y));
                let dst = self.new_container(kind, n + m)?;
                for i in 0..n {
                    let v = self.slot(x, i);
                    self.set_slot(dst, i, v);
                }
                for i in 0..m {
                    let v = self.slot(y, i);
                    self.set_slot(dst, n + i, v);
                }
                self.set_count(dst, n + m);
                Ok(Value::Obj(dst))
            }
            BinOp::Mul => {
                let (r, times) = match (self.seq(lhs), self.seq(rhs)) {
                    (Some(r), None) => (r, rhs.as_int().ok_or(Error::TypeError)?),
                    (None, Some(r)) => (r, lhs.as_int().ok_or(Error::TypeError)?),
                    _ => return Err(Error::TypeError),
                };
                let n = self.count(r);
                let times = times.max(0) as usize;
                let total = n.checked_mul(times).ok_or(Error::OutOfMemory)?;
                let dst = self.new_container(self.heap.kind(r), total)?;
                for k in 0..total {
                    let v = self.slot(r, k % n);
                    self.set_slot(dst, k, v);
                }
                self.set_count(dst, total);
                Ok(Value::Obj(dst))
            }
            _ => Err(Error::TypeError),
        }
    }

    // ---- iteration -----------------------------------------------------

    /// The next element of `it` (rooted) at position `state`, or `None` at
    /// the end. Works for lists, tuples, dicts (keys), ranges, strings and bytes.
    pub(super) fn iter_next(
        &mut self,
        it: Value<'a>,
        state: &mut usize,
    ) -> Result<Option<Value<'a>>, Error> {
        let i = *state;
        match it {
            Value::Obj(r) => {
                let kind = self.heap.kind(r);
                let next = match kind {
                    Kind::Range => {
                        let parts = range_parts(&self.heap.view(), r);
                        (i < range_len(parts)).then(|| Value::Int(parts.0 + i as i64 * parts.2))
                    }
                    Kind::Dict => (i < self.count(r)).then(|| self.slot(r, 2 * i)),
                    Kind::List | Kind::Tuple => (i < self.count(r)).then(|| self.slot(r, i)),
                    _ => return Err(Error::TypeError),
                };
                *state += 1;
                Ok(next)
            }
            Value::Str(_) | Value::HStr(_) => {
                let text = self.str_of(it).unwrap_or("");
                let Some(ch) = text[i..].chars().next() else {
                    return Ok(None);
                };
                *state += ch.len_utf8();
                self.substring(it, i, i + ch.len_utf8()).map(Some)
            }
            Value::Bytes(_) | Value::HBytes(_) => {
                let data = self.bytes_of(it).unwrap_or(&[]);
                *state += 1;
                Ok(data.get(i).map(|&b| Value::Int(b as i64)))
            }
            _ => Err(Error::TypeError),
        }
    }

    /// A new list with the elements of the iterable `it` (rooted).
    pub(super) fn collect_list(&mut self, it: Value<'a>, kind: Kind) -> Result<Value<'a>, Error> {
        let cap = self.container_len(it).unwrap_or(0);
        let r = self.new_container(Kind::List, cap)?;
        let base = self.sp;
        self.push(Value::Obj(r))?;
        let result = self.extend_list(r, it);
        self.sp = base;
        result?;
        if kind == Kind::Tuple {
            // Same layout: a tuple is a list that can't change
            self.heap.set_kind(r, Kind::Tuple);
        }
        Ok(Value::Obj(r))
    }

    /// `list.extend(it)`; `r` and `it` rooted.
    fn extend_list(&mut self, r: ObjRef, it: Value<'a>) -> Result<(), Error> {
        let mut state = 0;
        while let Some(v) = self.iter_next(it, &mut state)? {
            self.list_append(r, v)?;
        }
        Ok(())
    }

    /// Unpacks `v` (rooted) into exactly `out.len()` values.
    pub(super) fn unpack(&mut self, v: Value<'a>, out: &mut [Value<'a>]) -> Result<(), Error> {
        let mut state = 0;
        let mut n = 0;
        while let Some(x) = self.iter_next(v, &mut state)? {
            if n == out.len() {
                return Err(Error::ValueError); // too many values to unpack
            }
            out[n] = x;
            n += 1;
            // Elements made while unpacking a heap string must stay rooted
            self.push(x)?;
        }
        self.sp -= n;
        if n != out.len() {
            return Err(Error::ValueError); // not enough values to unpack
        }
        Ok(())
    }

    // ---- methods -------------------------------------------------------

    /// Methods of lists, tuples and dicts; `None` if `receiver` isn't one.
    pub(super) fn container_method(
        &mut self,
        receiver: Value<'a>,
        name: &str,
        args: &[Value<'a>],
    ) -> Option<Result<Value<'a>, Error>> {
        let Value::Obj(r) = receiver else {
            return None;
        };
        let kind = self.heap.kind(r);
        Some(match kind {
            Kind::List | Kind::Tuple => self.seq_method(r, kind, name, args),
            Kind::Dict => self.dict_method(r, name, args),
            _ => Err(Error::AttributeError),
        })
    }

    fn seq_method(
        &mut self,
        r: ObjRef,
        kind: Kind,
        name: &str,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        let n = self.count(r);
        match (name, args) {
            ("count", [x]) => Ok(Value::Int(
                (0..n).filter(|&i| self.equal(self.slot(r, i), *x)).count() as i64,
            )),
            ("index", [x]) => (0..n)
                .find(|&i| self.equal(self.slot(r, i), *x))
                .map(|i| Value::Int(i as i64))
                .ok_or(Error::ValueError),
            _ if kind == Kind::Tuple => Err(Error::AttributeError),
            ("append", [x]) => self.list_append(r, *x).map(|()| Value::None),
            ("extend", [it]) => self.extend_list(r, *it).map(|()| Value::None),
            ("insert", [at, x]) => {
                let at = at.as_int().ok_or(Error::TypeError)?;
                let at = (if at < 0 { at + n as i64 } else { at }).clamp(0, n as i64) as usize;
                self.list_append(r, *x)?;
                let v = self.slot(r, n);
                for i in (at..n).rev() {
                    let moved = self.slot(r, i);
                    self.set_slot(r, i + 1, moved);
                }
                self.set_slot(r, at, v);
                Ok(Value::None)
            }
            ("pop", [] | [_]) => {
                let i = match args.first() {
                    Some(i) => i.as_int().ok_or(Error::TypeError)?,
                    None => -1,
                };
                let i = norm_index(i, n).ok_or(Error::IndexError)?;
                let v = self.slot(r, i);
                self.list_remove_at(r, i);
                // Still reachable from the caller's stack slot? No: keep it
                // rooted by returning it right away (no allocation follows).
                Ok(v)
            }
            ("remove", [x]) => {
                let i = (0..n)
                    .find(|&i| self.equal(self.slot(r, i), *x))
                    .ok_or(Error::ValueError)?;
                self.list_remove_at(r, i);
                Ok(Value::None)
            }
            ("clear", []) => {
                self.set_count(r, 0);
                Ok(Value::None)
            }
            ("reverse", []) => {
                for i in 0..n / 2 {
                    let (a, b) = (self.slot(r, i), self.slot(r, n - 1 - i));
                    self.set_slot(r, i, b);
                    self.set_slot(r, n - 1 - i, a);
                }
                Ok(Value::None)
            }
            ("sort", []) => self.sort_list(r).map(|()| Value::None),
            ("copy", []) => self.container_slice(r, None, None),
            _ => Err(Error::AttributeError),
        }
    }

    /// Stable insertion sort with `<` (fine for the list sizes of an MCU).
    pub(super) fn sort_list(&mut self, r: ObjRef) -> Result<(), Error> {
        let n = self.count(r);
        for i in 1..n {
            let mut j = i;
            while j > 0 {
                let (a, b) = (self.slot(r, j - 1), self.slot(r, j));
                if !self.compare(CmpOp::Lt, b, a)? {
                    break;
                }
                self.set_slot(r, j - 1, b);
                self.set_slot(r, j, a);
                j -= 1;
            }
        }
        Ok(())
    }

    fn dict_method(
        &mut self,
        r: ObjRef,
        name: &str,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        let n = self.count(r);
        match (name, args) {
            ("get", [key] | [key, _]) => Ok(match self.dict_find(r, *key)? {
                Some(i) => self.slot(r, 2 * i + 1),
                None => args.get(1).copied().unwrap_or(Value::None),
            }),
            ("pop", [key] | [key, _]) => match self.dict_find(r, *key)? {
                Some(i) => {
                    let v = self.slot(r, 2 * i + 1);
                    self.dict_remove(r, i);
                    Ok(v)
                }
                None => args.get(1).copied().ok_or(Error::KeyError),
            },
            ("setdefault", [key] | [key, _]) => match self.dict_find(r, *key)? {
                Some(i) => Ok(self.slot(r, 2 * i + 1)),
                None => {
                    let v = args.get(1).copied().unwrap_or(Value::None);
                    self.dict_set(r, *key, v)?;
                    let i = self.count(r) - 1;
                    Ok(self.slot(r, 2 * i + 1))
                }
            },
            ("update", [other]) => {
                let src = self.dict(*other).ok_or(Error::TypeError)?;
                for i in 0..self.count(src) {
                    let (k, v) = (self.slot(src, 2 * i), self.slot(src, 2 * i + 1));
                    self.dict_set(r, k, v)?;
                }
                Ok(Value::None)
            }
            ("clear", []) => {
                self.set_count(r, 0);
                Ok(Value::None)
            }
            ("copy", []) => {
                let dst = self.new_container(Kind::Dict, n)?;
                for i in 0..2 * n {
                    let v = self.slot(r, i);
                    self.set_slot(dst, i, v);
                }
                self.set_count(dst, n);
                Ok(Value::Obj(dst))
            }
            // Views would need their own type: these return lists
            ("keys" | "values", []) => {
                let dst = self.new_container(Kind::List, n)?;
                let off = (name == "values") as usize;
                for i in 0..n {
                    let v = self.slot(r, 2 * i + off);
                    self.set_slot(dst, i, v);
                }
                self.set_count(dst, n);
                Ok(Value::Obj(dst))
            }
            ("items", []) => {
                let dst = self.new_container(Kind::List, n)?;
                let base = self.sp;
                self.push(Value::Obj(dst))?;
                let result = (|| {
                    for i in 0..n {
                        let pair = self.new_container(Kind::Tuple, 2)?;
                        let (k, v) = (self.slot(r, 2 * i), self.slot(r, 2 * i + 1));
                        self.set_slot(pair, 0, k);
                        self.set_slot(pair, 1, v);
                        self.set_count(pair, 2);
                        self.set_slot(dst, i, Value::Obj(pair));
                        self.set_count(dst, i + 1);
                    }
                    Ok(Value::Obj(dst))
                })();
                self.sp = base;
                result
            }
            _ => Err(Error::AttributeError),
        }
    }

    // ---- built-in functions ----------------------------------------------

    /// Built-ins for containers and iterables; `None` if `name` isn't one.
    pub(super) fn container_builtin(
        &mut self,
        name: &str,
        args: &[Value<'a>],
    ) -> Option<Result<Value<'a>, Error>> {
        Some(match (name, args) {
            ("list", []) => self.new_container(Kind::List, 0).map(Value::Obj),
            ("list", [it]) => self.collect_list(*it, Kind::List),
            ("tuple", []) => self.new_container(Kind::Tuple, 0).map(Value::Obj),
            ("tuple", [it]) => match self.obj_kind(*it) {
                Some(Kind::Tuple) => Ok(*it),
                _ => self.collect_list(*it, Kind::Tuple),
            },
            ("dict", []) => self.new_container(Kind::Dict, 0).map(Value::Obj),
            ("dict", [d]) => match self.dict(*d) {
                Some(r) => self.dict_method(r, "copy", &[]),
                None => Err(Error::TypeError),
            },
            ("range", [_] | [_, _] | [_, _, _]) => self.range(args),
            ("sorted", [it]) => self.collect_list(*it, Kind::List).and_then(|v| {
                let r = self.seq(v).ok_or(Error::TypeError)?;
                let base = self.sp;
                self.push(v)?;
                let result = self.sort_list(r);
                self.sp = base;
                result.map(|()| v)
            }),
            ("reversed", [it]) => self.collect_list(*it, Kind::List).inspect(|&v| {
                if let Some(r) = self.seq(v) {
                    let _ = self.seq_method(r, Kind::List, "reverse", &[]);
                }
            }),
            ("sum", [it] | [it, _]) => {
                let it = *it;
                (|| {
                    let mut total = match args.get(1) {
                        Some(v) => v.as_int().ok_or(Error::TypeError)?,
                        None => 0,
                    };
                    let mut state = 0;
                    while let Some(v) = self.iter_next(it, &mut state)? {
                        let n = v.as_int().ok_or(Error::TypeError)?;
                        total = total.checked_add(n).ok_or(Error::Overflow)?;
                    }
                    Ok(Value::Int(total))
                })()
            }
            ("any" | "all", [it]) => {
                let want = name == "any";
                let mut state = 0;
                loop {
                    match self.iter_next(*it, &mut state) {
                        Ok(Some(v)) if self.truthy(v) == want => break Ok(Value::Bool(want)),
                        Ok(Some(_)) => {}
                        Ok(None) => break Ok(Value::Bool(!want)),
                        Err(e) => break Err(e),
                    }
                }
            }
            ("min" | "max", [it]) => {
                let want = if name == "min" { CmpOp::Lt } else { CmpOp::Gt };
                let mut state = 0;
                let mut best: Option<Value<'a>> = None;
                loop {
                    match self.iter_next(*it, &mut state) {
                        Ok(Some(v)) => {
                            let better = match best {
                                None => Ok(true),
                                Some(b) => self.compare(want, v, b),
                            };
                            match better {
                                Ok(true) => {
                                    // Keep the candidate rooted (heap strings of a str)
                                    if best.is_some() {
                                        self.sp -= 1;
                                    }
                                    if let Err(e) = self.push(v) {
                                        break Err(e);
                                    }
                                    best = Some(v);
                                }
                                Ok(false) => {}
                                Err(e) => break Err(e),
                            }
                        }
                        Ok(None) => break best.ok_or(Error::ValueError),
                        Err(e) => break Err(e),
                    }
                }
                .inspect(|_| self.sp -= 1)
            }
            ("enumerate" | "zip", _) => self.pairs(name, args),
            _ => return None,
        })
    }

    fn range(&mut self, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        let mut n = [0i64; 3];
        for (slot, v) in n.iter_mut().zip(args) {
            *slot = v.as_int().ok_or(Error::TypeError)?;
        }
        let (start, stop, step) = match args.len() {
            1 => (0, n[0], 1),
            2 => (n[0], n[1], 1),
            _ => (n[0], n[1], n[2]),
        };
        if step == 0 {
            return Err(Error::ValueError);
        }
        let r = self.alloc_obj(Kind::Range, 24)?;
        let b = self.heap.bytes_mut(r);
        for (i, x) in [start, stop, step].iter().enumerate() {
            b[i * 8..i * 8 + 8].copy_from_slice(&x.to_le_bytes());
        }
        Ok(Value::Obj(r))
    }

    /// `enumerate(it, [start])` and `zip(a, b, ...)` as lists of tuples
    /// (there are no lazy iterators).
    fn pairs(&mut self, name: &str, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        let (sources, start) = match (name, args) {
            ("enumerate", [_]) => (&args[..1], 0),
            ("enumerate", [_, s]) => (&args[..1], s.as_int().ok_or(Error::TypeError)?),
            ("zip", [_, ..]) if args.len() <= 4 => (args, 0),
            _ => return Err(Error::TypeError),
        };
        let width = if name == "enumerate" {
            2
        } else {
            sources.len()
        };
        let dst = self.new_container(Kind::List, 0)?;
        let base = self.sp;
        self.push(Value::Obj(dst))?;
        let result = (|| {
            let mut states = [0usize; 4];
            let mut index = start;
            'outer: loop {
                let tuple = self.new_container(Kind::Tuple, width)?;
                self.push(Value::Obj(tuple))?;
                let mut k = 0;
                if name == "enumerate" {
                    self.set_slot(tuple, 0, Value::Int(index));
                    self.set_count(tuple, 1);
                    k = 1;
                }
                for (s, src) in sources.iter().enumerate() {
                    let Some(v) = self.iter_next(*src, &mut states[s])? else {
                        break 'outer;
                    };
                    let v = self.stored(v)?;
                    self.set_slot(tuple, k, v);
                    k += 1;
                    self.set_count(tuple, k);
                }
                self.list_append(dst, Value::Obj(tuple))?;
                self.sp -= 1;
                index += 1;
            }
            Ok(Value::Obj(dst))
        })();
        self.sp = base;
        result
    }

    // ---- garbage collection ----------------------------------------------

    /// Marks `v` and everything reachable from it.
    pub(super) fn mark_value(
        &mut self,
        v: Value<'a>,
        work: &mut [ObjRef; MAX_OBJECTS],
        n: &mut usize,
    ) {
        let r = match v {
            Value::HStr(r) | Value::HBytes(r) => {
                self.heap.mark(r);
                return;
            }
            Value::Obj(r) => r,
            _ => return,
        };
        if self.heap.mark(r) && self.heap.kind(r).is_container() {
            work[*n] = r;
            *n += 1;
        }
    }

    /// Marks the contents of the containers in `work` (and theirs).
    pub(super) fn trace(&mut self, work: &mut [ObjRef; MAX_OBJECTS], n: &mut usize) {
        while *n > 0 {
            *n -= 1;
            let r = work[*n];
            let slots = self.count(r)
                * if self.heap.kind(r) == Kind::Dict {
                    2
                } else {
                    1
                };
            for i in 0..slots {
                let v = self.slot(r, i);
                self.mark_value(v, work, n);
            }
        }
    }
}
