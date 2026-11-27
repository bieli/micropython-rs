// ============================================================================
// HEAP: fixed-size object memory with a compacting mark-and-sweep collector
// ============================================================================
//
// Objects created while a script runs (strings built at runtime, and later
// bytes, lists and dicts) live in a byte buffer given by the embedding
// application. There is no system allocator: when the buffer is full, the
// interpreter marks every object reachable from its roots (variables and
// the value stack), drops the rest, and moves the survivors together.
//
// Values refer to objects through `ObjRef` handles (an index into a fixed
// table), not through addresses, so moving an object's bytes during
// compaction never invalidates a value.

/// Objects the heap's own table has room for (`Interpreter::with_heap_table`
/// gives it any number, up to 65 535): 12 bytes each. The full level has room
/// for more: its frozen modules alone keep about 250 alive (`machine` 160,
/// `network` 65), while their bytes fill under a third of a 32 KB heap.
pub const MAX_OBJECTS: usize = crate::interpreter::level(256, 256, 512);

/// Containers the collector's work list holds at once; more are marked
/// "deferred" in the table and picked up afterwards, so the list (on the
/// stack) stays small whatever the table's size.
pub const MARK_WORK: usize = 64;

/// Handle of a heap object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjRef(u16);

impl ObjRef {
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// The handle with table index `i` (as stored inside containers).
    pub const fn from_index(i: usize) -> Self {
        Self(i as u16)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Free,
    /// UTF-8 text.
    Str,
    /// Raw bytes.
    Bytes,
    /// Containers: a length (u32) followed by encoded values (see
    /// `interpreter/containers.rs`); a dict stores key/value pairs.
    List,
    Tuple,
    Dict,
    /// `range(start, stop, step)`: three i64.
    Range,
    /// An exception instance: laid out like a tuple of its class number
    /// followed by its arguments.
    Exception,
    /// A file object (see `interpreter/files.rs`), laid out like a tuple.
    File,
    /// A method taken from an object without calling it (`r = f.read`):
    /// a tuple of the object and the method name.
    BoundMethod,
    /// Classes and instances (see `interpreter/classes.rs`), `classmethod` /
    /// `staticmethod` wrappers and `super()` objects, all laid out like tuples.
    Class,
    Instance,
    ClassMethod,
    StaticMethod,
    Super,
    /// A function made inside a function: `(function, dict of the values
    /// of the enclosing function's variables when it was made)`.
    Closure,
    /// An imported `.py` module: `(name, dict)`.
    PyModule,
}

impl Kind {
    /// True for objects whose bytes hold references to other objects.
    pub const fn is_container(self) -> bool {
        matches!(
            self,
            Kind::List
                | Kind::Tuple
                | Kind::Dict
                | Kind::Exception
                | Kind::File
                | Kind::BoundMethod
                | Kind::Class
                | Kind::Instance
                | Kind::ClassMethod
                | Kind::StaticMethod
                | Kind::Super
                | Kind::Closure
                | Kind::PyModule
        )
    }
}

/// One entry of the heap's object table (`Interpreter::with_heap_table`).
#[derive(Debug, Clone, Copy)]
pub struct HeapEntry {
    offset: u32,
    len: u32,
    kind: Kind,
    marked: bool,
    /// While marking: 1 for a container whose contents are still to be
    /// marked (it didn't fit the work list). While compacting: the sort order.
    aux: u16,
}

impl HeapEntry {
    pub const FREE: Self = Self {
        offset: 0,
        len: 0,
        kind: Kind::Free,
        marked: false,
        aux: 0,
    };
}

type Entry = HeapEntry;
const FREE: Entry = HeapEntry::FREE;

/// The object table: the heap's own, or the embedder's (any size).
#[allow(clippy::large_enum_variant)]
enum Table<'a> {
    Inline([Entry; MAX_OBJECTS]),
    External(&'a mut [Entry]),
}

pub struct Heap<'a> {
    mem: &'a mut [u8],
    /// Bytes in use from the start of `mem` (objects are packed below it).
    top: usize,
    /// Number of collections so far (for tests and diagnostics).
    pub collections: u32,
    table: Table<'a>,
    /// Live objects (table entries in use)
    live: usize,
    /// Where `alloc` looks for a free entry first
    free_hint: usize,
    /// Containers marked but not traced yet (`defer`), and where to look next
    deferred: usize,
    defer_scan: usize,
    /// Entries at and above this are free: collections look only below it
    /// (a table of 65 535 entries in slow external RAM, with a few hundred
    /// objects alive, costs a few ms instead of a hundred)
    high: usize,
}

impl<'a> Heap<'a> {
    pub fn new(mem: &'a mut [u8]) -> Self {
        Self {
            mem,
            top: 0,
            table: Table::Inline([FREE; MAX_OBJECTS]),
            collections: 0,
            live: 0,
            free_hint: 0,
            deferred: 0,
            defer_scan: 0,
            high: 0,
        }
    }

    /// Uses `table` (up to 65 535 entries) instead of the heap's own; the
    /// objects so far move over (as far as they fit).
    pub fn set_table(&mut self, table: &'a mut [Entry]) {
        let len = table.len().min(u16::MAX as usize);
        let table = &mut table[..len];
        table.fill(FREE);
        for (new, old) in table.iter_mut().zip(self.entries().iter()) {
            *new = *old;
        }
        self.live = table.iter().filter(|e| e.kind != Kind::Free).count();
        self.high = table
            .iter()
            .rposition(|e| e.kind != Kind::Free)
            .map_or(0, |i| i + 1);
        self.table = Table::External(table);
        self.free_hint = 0;
    }

    #[inline(always)]
    fn entries(&self) -> &[Entry] {
        match &self.table {
            Table::Inline(t) => t,
            Table::External(t) => t,
        }
    }

    #[inline(always)]
    fn entries_mut(&mut self) -> &mut [Entry] {
        match &mut self.table {
            Table::Inline(t) => t,
            Table::External(t) => t,
        }
    }

    /// Entries in the object table (the most objects alive at once).
    pub fn table_len(&self) -> usize {
        self.entries().len()
    }

    pub fn capacity(&self) -> usize {
        self.mem.len()
    }

    /// Bytes used by objects (including garbage not yet collected).
    pub fn used(&self) -> usize {
        self.top
    }

    pub fn objects(&self) -> usize {
        self.live
    }

    /// True if an object of `len` bytes fits without collecting.
    pub fn fits(&self, len: usize) -> bool {
        self.mem.len() - self.top >= len && self.live < self.table_len()
    }

    /// Like `alloc`, without zeroing the bytes (they keep what was there):
    /// a scratch area, written before it's read.
    pub fn alloc_unzeroed(&mut self, kind: Kind, len: usize) -> Option<ObjRef> {
        self.alloc_raw(kind, len)
    }

    /// Allocates `len` zeroed bytes without collecting; `None` if it doesn't fit.
    pub fn alloc(&mut self, kind: Kind, len: usize) -> Option<ObjRef> {
        let top = self.top;
        let r = self.alloc_raw(kind, len)?;
        self.mem[top..top + len].fill(0);
        Some(r)
    }

    fn alloc_raw(&mut self, kind: Kind, len: usize) -> Option<ObjRef> {
        if self.mem.len() - self.top < len || self.live >= self.table_len() {
            return None;
        }
        // A free entry, from the hint on (entries below it were in use)
        let start = self.free_hint;
        let table = self.entries();
        let index = (start..table.len())
            .chain(0..start)
            .find(|&i| table[i].kind == Kind::Free)?;
        self.entries_mut()[index] = Entry {
            offset: self.top as u32,
            len: len as u32,
            kind,
            marked: false,
            aux: 0,
        };
        self.live += 1;
        self.free_hint = index + 1;
        self.high = self.high.max(index + 1);
        self.top += len;
        Some(ObjRef(index as u16))
    }

    pub fn kind(&self, r: ObjRef) -> Kind {
        self.entries()[r.0 as usize].kind
    }

    /// Changes the kind of a live object (e.g. a finished list into a tuple).
    pub fn set_kind(&mut self, r: ObjRef, kind: Kind) {
        self.entries_mut()[r.0 as usize].kind = kind;
    }

    pub fn bytes(&self, r: ObjRef) -> &[u8] {
        let e = self.entries()[r.0 as usize];
        &self.mem[e.offset as usize..(e.offset + e.len) as usize]
    }

    /// Changes the size of an object, keeping its handle and its contents
    /// (new bytes are zeroed). Without collecting; `false` if it doesn't fit.
    /// A grown object that isn't the last one moves to the top; its old
    /// bytes are reclaimed by the next collection.
    pub fn realloc(&mut self, r: ObjRef, new_len: usize) -> bool {
        let e = self.entries()[r.0 as usize];
        let (off, len) = (e.offset as usize, e.len as usize);
        if off + len == self.top {
            if off + new_len > self.mem.len() {
                return false;
            }
            if new_len > len {
                self.mem[off + len..off + new_len].fill(0);
            }
            self.top = off + new_len;
        } else if new_len > len {
            if self.mem.len() - self.top < new_len {
                return false;
            }
            let at = self.top;
            self.mem.copy_within(off..off + len, at);
            self.mem[at + len..at + new_len].fill(0);
            self.entries_mut()[r.0 as usize].offset = at as u32;
            self.top = at + new_len;
        }
        self.entries_mut()[r.0 as usize].len = new_len as u32;
        true
    }

    /// Read-only access to all objects.
    pub fn view(&self) -> HeapView<'_> {
        let table = match &self.table {
            Table::Inline(t) => &t[..],
            Table::External(t) => &t[..],
        };
        HeapView {
            mem: self.mem,
            table,
        }
    }

    /// Reads all other objects while writing `dst`, which must be the most
    /// recently allocated object (so every other object lies below it).
    pub fn view_and_dst(&mut self, dst: ObjRef) -> (HeapView<'_>, &mut [u8]) {
        let e = self.entries()[dst.0 as usize];
        let table = match &self.table {
            Table::Inline(t) => &t[..],
            Table::External(t) => &t[..],
        };
        let (low, high) = self.mem.split_at_mut(e.offset as usize);
        (HeapView { mem: low, table }, &mut high[..e.len as usize])
    }

    pub fn bytes_mut(&mut self, r: ObjRef) -> &mut [u8] {
        let e = self.entries()[r.0 as usize];
        &mut self.mem[e.offset as usize..(e.offset + e.len) as usize]
    }

    /// The text of a string object.
    pub fn str(&self, r: ObjRef) -> &str {
        // Only valid UTF-8 is ever stored in string objects
        core::str::from_utf8(self.bytes(r)).unwrap_or("")
    }

    /// Copies part of another object (`src[from..from + len]`) into `dst` at `at`.
    pub fn copy_obj(&mut self, src: ObjRef, from: usize, dst: ObjRef, at: usize, len: usize) {
        let s = self.entries()[src.0 as usize].offset as usize + from;
        let d = self.entries()[dst.0 as usize].offset as usize + at;
        self.mem.copy_within(s..s + len, d);
    }

    /// Reads `src` while writing `dst` (two different objects never overlap).
    pub fn src_dst(&mut self, src: ObjRef, dst: ObjRef) -> (&[u8], &mut [u8]) {
        let s = self.entries()[src.0 as usize];
        let d = self.entries()[dst.0 as usize];
        let (s0, s1) = (s.offset as usize, (s.offset + s.len) as usize);
        let (d0, d1) = (d.offset as usize, (d.offset + d.len) as usize);
        if s0 < d0 {
            let (low, high) = self.mem.split_at_mut(d0);
            (&low[s0..s1], &mut high[..d1 - d0])
        } else {
            let (low, high) = self.mem.split_at_mut(s0);
            (&high[..s1 - s0], &mut low[d0..d1])
        }
    }

    /// Starts a collection: clears all marks.
    pub fn begin_mark(&mut self) {
        let high = self.high;
        for e in self.entries_mut()[..high].iter_mut() {
            e.marked = false;
            e.aux = 0;
        }
        self.deferred = 0;
        self.defer_scan = 0;
    }

    /// Marks an object as reachable; `true` if it wasn't marked yet.
    pub fn mark(&mut self, r: ObjRef) -> bool {
        let e = &mut self.entries_mut()[r.0 as usize];
        let new = !e.marked;
        e.marked = true;
        new
    }

    /// A marked container whose contents the work list has no room for:
    /// `take_deferred` hands it out later.
    pub fn defer(&mut self, r: ObjRef) {
        self.entries_mut()[r.0 as usize].aux = 1;
        self.deferred += 1;
    }

    /// A deferred container (and forgets it), or `None` when none is left.
    pub fn take_deferred(&mut self) -> Option<ObjRef> {
        if self.deferred == 0 {
            return None;
        }
        // Round the table from where the last one was found
        let len = self.high;
        let start = self.defer_scan.min(len);
        let table = self.entries();
        let i = (start..len).chain(0..start).find(|&i| table[i].aux == 1)?;
        self.entries_mut()[i].aux = 0;
        self.deferred -= 1;
        self.defer_scan = i;
        Some(ObjRef(i as u16))
    }

    /// Frees unmarked objects and moves the marked ones to the start of the
    /// buffer, keeping their order (and their handles).
    pub fn sweep(&mut self) {
        self.collections += 1;
        let mut live = 0;
        let mut last = 0;
        let high = self.high;
        for (i, e) in self.entries_mut()[..high].iter_mut().enumerate() {
            if e.kind != Kind::Free && !e.marked {
                *e = FREE;
            } else if e.kind != Kind::Free {
                live += 1;
                last = i + 1;
            }
        }
        self.live = live;
        self.high = last;
        self.free_hint = 0;
        // The survivors in address order: their indices, in the `aux` fields
        // of the first `live` entries (no other memory), heap-sorted by offset
        let n = live;
        if !self.radix_order(last, n) {
            let table = self.entries_mut();
            let mut k = 0;
            for i in 0..last {
                if table[i].kind != Kind::Free {
                    table[k].aux = i as u16;
                    k += 1;
                }
            }
            sort_by_offset(table, n);
        }
        let mut top = 0;
        for k in 0..n {
            let i = self.entries()[k].aux as usize;
            let e = self.entries()[i];
            let (from, len) = (e.offset as usize, e.len as usize);
            if from != top {
                self.mem.copy_within(from..from + len, top);
                self.entries_mut()[i].offset = top as u32;
            }
            top += len;
        }
        self.top = top;
    }
}

impl Heap<'_> {
    /// The survivors' address order (into `table[..n].aux`, as `sweep` needs
    /// it) by a radix sort of (offset, index) pairs in the free memory above
    /// `top`: a few passes that read and write memory in order, much faster
    /// than the heapsort's scattered reads when the table is in slow
    /// external RAM. `false` if the free memory can't hold them (16 bytes a
    /// survivor; the heapsort is then used).
    fn radix_order(&mut self, last: usize, n: usize) -> bool {
        let need = 16 * n;
        if n < 64 || self.mem.len() - self.top < need {
            return false;
        }
        let (table, mem, top) = match &mut self.table {
            Table::Inline(t) => (&mut t[..], &mut *self.mem, self.top),
            Table::External(t) => (&mut t[..], &mut *self.mem, self.top),
        };
        let (a, b) = mem[top..top + need].split_at_mut(8 * n);
        let mut k = 0;
        for (i, e) in table[..last].iter().enumerate() {
            if e.kind != Kind::Free {
                a[8 * k..8 * k + 4].copy_from_slice(&e.offset.to_le_bytes());
                a[8 * k + 4..8 * k + 8].copy_from_slice(&(i as u32).to_le_bytes());
                k += 1;
            }
        }
        // Offsets are below the heap's size: that many bytes of them sorted
        let bits = 32 - (mem_len_bits(top));
        let passes = bits.div_ceil(8).max(1);
        let (mut src, mut dst) = (a, b);
        for pass in 0..passes as usize {
            // The offset's byte `pass` (little-endian: the least significant first)
            let byte = pass;
            let mut count = [0usize; 256];
            for k in 0..n {
                count[src[8 * k + byte] as usize] += 1;
            }
            let mut at = 0;
            for c in count.iter_mut() {
                let here = *c;
                *c = at;
                at += here;
            }
            for k in 0..n {
                let digit = src[8 * k + byte] as usize;
                let to = count[digit];
                count[digit] += 1;
                dst[8 * to..8 * to + 8].copy_from_slice(&src[8 * k..8 * k + 8]);
            }
            core::mem::swap(&mut src, &mut dst);
        }
        for (k, pair) in src[..8 * n].as_chunks::<8>().0.iter().enumerate() {
            table[k].aux = u32::from_le_bytes([pair[4], pair[5], pair[6], pair[7]]) as u16;
        }
        true
    }
}

/// Leading zero bits of the largest offset there can be (below `top`).
fn mem_len_bits(top: usize) -> u32 {
    (top.max(1) as u32).leading_zeros()
}

/// Heapsort of `table[..n].aux` (entry indices) by those entries' offsets:
/// O(n log n) and in place, for tables of any size.
fn sort_by_offset(table: &mut [Entry], n: usize) {
    fn key(t: &[Entry], k: usize) -> u32 {
        t[t[k].aux as usize].offset
    }
    fn sift(t: &mut [Entry], mut root: usize, end: usize) {
        loop {
            let mut child = 2 * root + 1;
            if child >= end {
                break;
            }
            if child + 1 < end && key(t, child + 1) > key(t, child) {
                child += 1;
            }
            if key(t, root) >= key(t, child) {
                break;
            }
            let (a, b) = (t[root].aux, t[child].aux);
            t[root].aux = b;
            t[child].aux = a;
            root = child;
        }
    }
    for start in (0..n / 2).rev() {
        sift(table, start, n);
    }
    for end in (1..n).rev() {
        let (a, b) = (table[0].aux, table[end].aux);
        table[0].aux = b;
        table[end].aux = a;
        sift(table, 0, end);
    }
}

/// Read-only view of the heap's objects (see `Heap::view_and_dst`).
#[derive(Clone, Copy)]
pub struct HeapView<'h> {
    mem: &'h [u8],
    table: &'h [Entry],
}

impl<'h> HeapView<'h> {
    pub fn kind(&self, r: ObjRef) -> Kind {
        self.table[r.0 as usize].kind
    }

    pub fn bytes(&self, r: ObjRef) -> &'h [u8] {
        let e = self.table[r.0 as usize];
        &self.mem[e.offset as usize..(e.offset + e.len) as usize]
    }

    pub fn str(&self, r: ObjRef) -> &'h str {
        core::str::from_utf8(self.bytes(r)).unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alloc_compact_keeps_handles() {
        let mut mem = [0u8; 64];
        let mut heap = Heap::new(&mut mem);
        let a = heap.alloc(Kind::Str, 10).unwrap();
        heap.bytes_mut(a).copy_from_slice(b"aaaaaaaaaa");
        let b = heap.alloc(Kind::Str, 20).unwrap();
        heap.bytes_mut(b).copy_from_slice(&[b'b'; 20]);
        let c = heap.alloc(Kind::Str, 30).unwrap();
        heap.bytes_mut(c).copy_from_slice(&[b'c'; 30]);
        assert_eq!(heap.used(), 60);
        assert!(heap.alloc(Kind::Str, 10).is_none());
        assert!(!heap.fits(10));

        // Keep a and c: b's space is reclaimed and c moves down
        heap.begin_mark();
        heap.mark(a);
        heap.mark(c);
        heap.sweep();
        assert_eq!(heap.used(), 40);
        assert_eq!(heap.objects(), 2);
        assert_eq!(heap.str(a), "aaaaaaaaaa");
        assert_eq!(heap.bytes(c), &[b'c'; 30]);
        assert_eq!(heap.kind(b), Kind::Free);

        let d = heap.alloc(Kind::Str, 24).unwrap();
        heap.copy_obj(a, 2, d, 0, 8);
        assert_eq!(&heap.bytes(d)[..8], b"aaaaaaaa");
        // Read one object while writing another, in both address orders
        let (src, dst) = heap.src_dst(c, d);
        dst[8..18].copy_from_slice(&src[..10]);
        assert_eq!(&heap.bytes(d)[8..18], &[b'c'; 10]);
        let (src, dst) = heap.src_dst(d, a);
        dst[..2].copy_from_slice(&src[..2]);
        assert_eq!(heap.str(a), "aaaaaaaaaa");
        assert_eq!(heap.capacity(), 64);
        assert_eq!(heap.collections, 1);
    }

    #[test]
    fn test_realloc_keeps_handle_and_contents() {
        let mut mem = [0u8; 64];
        let mut heap = Heap::new(&mut mem);
        let a = heap.alloc(Kind::List, 4).unwrap();
        heap.bytes_mut(a).copy_from_slice(b"abcd");
        // The last object grows in place
        assert!(heap.realloc(a, 8));
        assert_eq!(heap.bytes(a), b"abcd\0\0\0\0");
        assert_eq!(heap.used(), 8);
        // Not the last one any more: it moves to the top
        let b = heap.alloc(Kind::Str, 4).unwrap();
        assert!(heap.realloc(a, 12));
        assert_eq!(&heap.bytes(a)[..4], b"abcd");
        assert_eq!(heap.used(), 24);
        assert!(!heap.realloc(b, 64));
        // Its old bytes are garbage, reclaimed by compaction
        heap.begin_mark();
        assert!(heap.mark(a));
        assert!(!heap.mark(a));
        heap.mark(b);
        heap.sweep();
        assert_eq!(heap.used(), 16);
        assert_eq!(&heap.bytes(a)[..4], b"abcd");
        // Writing the newest object while reading the others
        let c = heap.alloc(Kind::Str, 2).unwrap();
        let (view, out) = heap.view_and_dst(c);
        out.copy_from_slice(&view.bytes(a)[..2]);
        assert_eq!(view.kind(a), Kind::List);
        assert_eq!(heap.str(c), "ab");
    }

    #[test]
    fn test_object_table_limit() {
        let mut mem = [0u8; 4096];
        let mut heap = Heap::new(&mut mem);
        for _ in 0..MAX_OBJECTS {
            heap.alloc(Kind::Str, 1).unwrap();
        }
        assert!(heap.alloc(Kind::Str, 1).is_none());
        heap.begin_mark();
        heap.sweep();
        assert_eq!(heap.objects(), 0);
        assert_eq!(heap.used(), 0);
        assert!(heap.alloc(Kind::Str, 0).is_some());
    }
}
