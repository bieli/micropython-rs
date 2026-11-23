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

/// Maximum number of objects alive at the same time.
pub const MAX_OBJECTS: usize = 256;

/// Handle of a heap object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjRef(u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Free,
    /// UTF-8 text.
    Str,
    /// Raw bytes.
    Bytes,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    offset: u32,
    len: u32,
    kind: Kind,
    marked: bool,
}

const FREE: Entry = Entry {
    offset: 0,
    len: 0,
    kind: Kind::Free,
    marked: false,
};

pub struct Heap<'a> {
    mem: &'a mut [u8],
    /// Bytes in use from the start of `mem` (objects are packed below it).
    top: usize,
    /// Number of collections so far (for tests and diagnostics).
    pub collections: u32,
    table: [Entry; MAX_OBJECTS],
}

impl<'a> Heap<'a> {
    pub fn new(mem: &'a mut [u8]) -> Self {
        Self {
            mem,
            top: 0,
            table: [FREE; MAX_OBJECTS],
            collections: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.mem.len()
    }

    /// Bytes used by objects (including garbage not yet collected).
    pub fn used(&self) -> usize {
        self.top
    }

    pub fn objects(&self) -> usize {
        self.table.iter().filter(|e| e.kind != Kind::Free).count()
    }

    /// True if an object of `len` bytes fits without collecting.
    pub fn fits(&self, len: usize) -> bool {
        self.mem.len() - self.top >= len && self.table.iter().any(|e| e.kind == Kind::Free)
    }

    /// Allocates `len` zeroed bytes without collecting; `None` if it doesn't fit.
    pub fn alloc(&mut self, kind: Kind, len: usize) -> Option<ObjRef> {
        if self.mem.len() - self.top < len {
            return None;
        }
        let index = self.table.iter().position(|e| e.kind == Kind::Free)?;
        self.table[index] = Entry {
            offset: self.top as u32,
            len: len as u32,
            kind,
            marked: false,
        };
        self.mem[self.top..self.top + len].fill(0);
        self.top += len;
        Some(ObjRef(index as u16))
    }

    pub fn kind(&self, r: ObjRef) -> Kind {
        self.table[r.0 as usize].kind
    }

    pub fn bytes(&self, r: ObjRef) -> &[u8] {
        let e = self.table[r.0 as usize];
        &self.mem[e.offset as usize..(e.offset + e.len) as usize]
    }

    pub fn bytes_mut(&mut self, r: ObjRef) -> &mut [u8] {
        let e = self.table[r.0 as usize];
        &mut self.mem[e.offset as usize..(e.offset + e.len) as usize]
    }

    /// The text of a string object.
    pub fn str(&self, r: ObjRef) -> &str {
        // Only valid UTF-8 is ever stored in string objects
        core::str::from_utf8(self.bytes(r)).unwrap_or("")
    }

    /// Copies part of another object (`src[from..from + len]`) into `dst` at `at`.
    pub fn copy_obj(&mut self, src: ObjRef, from: usize, dst: ObjRef, at: usize, len: usize) {
        let s = self.table[src.0 as usize].offset as usize + from;
        let d = self.table[dst.0 as usize].offset as usize + at;
        self.mem.copy_within(s..s + len, d);
    }

    /// Reads `src` while writing `dst` (two different objects never overlap).
    pub fn src_dst(&mut self, src: ObjRef, dst: ObjRef) -> (&[u8], &mut [u8]) {
        let s = self.table[src.0 as usize];
        let d = self.table[dst.0 as usize];
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
        for e in self.table.iter_mut() {
            e.marked = false;
        }
    }

    pub fn mark(&mut self, r: ObjRef) {
        self.table[r.0 as usize].marked = true;
    }

    /// Frees unmarked objects and moves the marked ones to the start of the
    /// buffer, keeping their order (and their handles).
    pub fn sweep(&mut self) {
        self.collections += 1;
        for e in self.table.iter_mut() {
            if e.kind != Kind::Free && !e.marked {
                *e = FREE;
            }
        }
        // Survivors in address order (insertion sort: at most MAX_OBJECTS)
        let mut order = [0u16; MAX_OBJECTS];
        let mut n = 0;
        for (i, e) in self.table.iter().enumerate() {
            if e.kind != Kind::Free {
                let mut j = n;
                while j > 0 && self.table[order[j - 1] as usize].offset > e.offset {
                    order[j] = order[j - 1];
                    j -= 1;
                }
                order[j] = i as u16;
                n += 1;
            }
        }
        let mut top = 0;
        for &i in &order[..n] {
            let e = &mut self.table[i as usize];
            let (from, len) = (e.offset as usize, e.len as usize);
            if from != top {
                self.mem.copy_within(from..from + len, top);
                e.offset = top as u32;
            }
            top += len;
        }
        self.top = top;
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
