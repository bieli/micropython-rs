// ============================================================================
// FLATFS: a tiny flat file system for NOR flash (no heap, no directories)
// ============================================================================
//
// The flash region is split into fixed slots of `SLOT_SIZE` bytes; each file
// takes one slot. A slot starts with a 64-byte header followed by the data:
//
//     offset  size  field
//          0     4  magic  "MRFS" (written to 0 to mark the slot dead)
//          4     4  generation (grows with every write, newest version wins)
//          8     1  name length
//         12    40  name
//         52     4  size     (written by `finish`)
//         56     4  checksum (FNV-1a of the data, written by `finish`)
//         60     4  commit: 0xFFFFFFFF while writing, 0 once complete
//
// NOR flash can only clear bits between erases, so the header is filled in
// stages into still-erased bytes, and the commit word is written last.
// A new version always goes to a *different*, freshly erased slot; the old
// one is marked dead only after the new one is committed. A power cut while
// saving therefore leaves the previous version intact. Writing to the next
// free slot after the most recent one also spreads wear over the region.

/// Flash access needed by the file system, in absolute region offsets.
pub trait Flash {
    /// Size of the region in bytes (a multiple of `SECTOR_SIZE`).
    fn size(&self) -> u32;
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), FsError>;
    /// Writes into erased flash. `offset` and `data.len()` are multiples of 4.
    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), FsError>;
    /// Erases `len` bytes; `offset` and `len` are multiples of `SECTOR_SIZE`.
    fn erase(&mut self, offset: u32, len: u32) -> Result<(), FsError>;
}

pub const SECTOR_SIZE: u32 = 4096;
/// One file per slot: 8 sectors.
pub const SLOT_SIZE: u32 = 8 * SECTOR_SIZE;
const HEADER_SIZE: u32 = 64;
/// Largest file that fits in a slot.
pub const MAX_FILE_SIZE: u32 = SLOT_SIZE - HEADER_SIZE;
pub const MAX_NAME_LEN: usize = 40;

const MAGIC: [u8; 4] = *b"MRFS";
const OFF_GENERATION: u32 = 4;
const OFF_NAME_LEN: u32 = 8;
const OFF_NAME: u32 = 12;
const OFF_SIZE: u32 = 52;
const OFF_COMMIT: u32 = 60;
/// Data is written in chunks of this size (a multiple of 4).
const WRITE_CHUNK: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    /// The flash driver failed.
    Io,
    NotFound,
    /// No free slot left.
    NoSpace,
    /// The file is larger than `MAX_FILE_SIZE`.
    TooLarge,
    /// Empty, too long, or with characters outside `A-Z a-z 0-9 . _ -`.
    BadName,
    /// The stored data doesn't match its checksum.
    Corrupt,
    /// The region is smaller than one slot.
    NoRegion,
    /// Writing to a file that's closed (or was replaced by another open file).
    NotOpen,
}

impl FsError {
    /// Text for error messages, in the style of MicroPython's `OSError`.
    pub const fn message(self) -> &'static str {
        match self {
            FsError::Io => "flash I/O error",
            FsError::NotFound => "no such file",
            FsError::NoSpace => {
                "no free file slot (delete a file: saving needs one free slot, even to replace)"
            }
            FsError::TooLarge => "file too large (max 32704 bytes)",
            FsError::BadName => "bad file name (use A-Z a-z 0-9 . _ -, max 40 chars)",
            FsError::Corrupt => "file corrupted (checksum mismatch)",
            FsError::NoRegion => "no file system partition",
            FsError::NotOpen => "file not open for writing",
        }
    }
}

/// A committed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileInfo {
    pub slot: u32,
    pub size: u32,
    checksum: u32,
    generation: u32,
    name: [u8; MAX_NAME_LEN],
    name_len: u8,
}

impl FileInfo {
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// Free, dead, unfinished or foreign data: erased before reuse.
    Unused {
        generation: u32,
    },
    File(FileInfo),
}

/// A file being written; finish it with `FlatFs::finish`.
pub struct Writer {
    slot: u32,
    generation: u32,
    name: [u8; MAX_NAME_LEN],
    name_len: u8,
    size: u32,
    checksum: u32,
    buf: [u8; WRITE_CHUNK],
    buf_len: usize,
}

impl Writer {
    pub fn size(&self) -> u32 {
        self.size + self.buf_len as u32
    }
}

pub struct FlatFs<F: Flash> {
    flash: F,
    slots: u32,
    next_generation: u32,
    /// Slot to try first for the next write (round robin for wear leveling).
    cursor: u32,
    /// The file a script has open for writing (see `FileSystem`), and the
    /// token its file object holds.
    open: Option<Writer>,
    open_token: u32,
}

impl<F: Flash> FlatFs<F> {
    /// Scans the region. Stale duplicates left by an interrupted replace are
    /// marked dead here.
    pub fn mount(flash: F) -> Result<Self, FsError> {
        let slots = flash.size() / SLOT_SIZE;
        if slots == 0 {
            return Err(FsError::NoRegion);
        }
        let mut fs = Self {
            flash,
            slots,
            next_generation: 1,
            cursor: 0,
            open: None,
            open_token: 0,
        };

        let mut newest = (0, 0); // (generation, slot)
        for slot in 0..slots {
            let generation = match fs.slot(slot)? {
                Slot::File(info) => info.generation,
                Slot::Unused { generation } => generation,
            };
            if generation >= newest.0 {
                newest = (generation, slot);
            }
        }
        fs.next_generation = newest.0.wrapping_add(1).max(1);
        fs.cursor = (newest.1 + 1) % slots;

        // Two committed versions of one name: keep the newest
        for slot in 0..slots {
            if let Slot::File(info) = fs.slot(slot)? {
                if let Some(newer) = fs.find(info.name())? {
                    if newer.slot != slot {
                        fs.kill(slot)?;
                    }
                }
            }
        }
        Ok(fs)
    }

    pub fn into_flash(self) -> F {
        self.flash
    }

    /// The flash device, e.g. for data outside the file system's slots.
    pub fn flash_mut(&mut self) -> &mut F {
        &mut self.flash
    }

    /// Number of slots: the maximum number of files.
    pub fn slot_count(&self) -> u32 {
        self.slots
    }

    /// Calls `f` for every file.
    pub fn list(&mut self, mut f: impl FnMut(&FileInfo)) -> Result<(), FsError> {
        for slot in 0..self.slots {
            if let Slot::File(info) = self.slot(slot)? {
                // Skip an older duplicate (only possible right after a crash)
                if self
                    .find(info.name())?
                    .is_some_and(|newest| newest.slot == slot)
                {
                    f(&info);
                }
            }
        }
        Ok(())
    }

    /// `(files, slots)`: used and total slots.
    pub fn usage(&mut self) -> Result<(u32, u32), FsError> {
        let mut used = 0;
        self.list(|_| used += 1)?;
        Ok((used, self.slots))
    }

    /// The newest committed version of `name`.
    pub fn find(&mut self, name: &str) -> Result<Option<FileInfo>, FsError> {
        let mut best: Option<FileInfo> = None;
        for slot in 0..self.slots {
            if let Slot::File(info) = self.slot(slot)? {
                if info.name() == name && best.is_none_or(|b| info.generation > b.generation) {
                    best = Some(info);
                }
            }
        }
        Ok(best)
    }

    /// Reads part of a file; returns the number of bytes read (0 at the end).
    pub fn read_at(
        &mut self,
        file: &FileInfo,
        offset: u32,
        buf: &mut [u8],
    ) -> Result<usize, FsError> {
        if offset >= file.size {
            return Ok(0);
        }
        let len = buf.len().min((file.size - offset) as usize);
        let start = file.slot * SLOT_SIZE + HEADER_SIZE + offset;
        self.flash.read(start, &mut buf[..len])?;
        Ok(len)
    }

    /// Reads a whole file into `dst` and checks its checksum.
    pub fn read(&mut self, name: &str, dst: &mut [u8]) -> Result<usize, FsError> {
        let file = self.find(name)?.ok_or(FsError::NotFound)?;
        if file.size as usize > dst.len() {
            return Err(FsError::TooLarge);
        }
        let len = self.read_at(&file, 0, dst)?;
        if fnv1a(FNV_START, &dst[..len]) != file.checksum {
            return Err(FsError::Corrupt);
        }
        Ok(len)
    }

    /// Writes a whole file (replacing an older version).
    pub fn write(&mut self, name: &str, data: &[u8]) -> Result<(), FsError> {
        let mut writer = self.begin(name)?;
        self.append(&mut writer, data)?;
        self.finish(writer)
    }

    /// Starts writing `name` into a freshly erased slot. The old version, if
    /// any, stays readable until `finish`.
    pub fn begin(&mut self, name: &str) -> Result<Writer, FsError> {
        check_name(name)?;
        let slot = self.free_slot()?;
        let base = slot * SLOT_SIZE;
        self.flash.erase(base, SLOT_SIZE)?;

        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.cursor = (slot + 1) % self.slots;

        let mut head = [0xFFu8; OFF_SIZE as usize];
        head[..4].copy_from_slice(&MAGIC);
        head[4..8].copy_from_slice(&generation.to_le_bytes());
        head[OFF_NAME_LEN as usize] = name.len() as u8;
        let name_start = OFF_NAME as usize;
        head[name_start..name_start + name.len()].copy_from_slice(name.as_bytes());
        self.flash.write(base, &head)?;

        let mut stored = [0u8; MAX_NAME_LEN];
        stored[..name.len()].copy_from_slice(name.as_bytes());
        Ok(Writer {
            slot,
            generation,
            name: stored,
            name_len: name.len() as u8,
            size: 0,
            checksum: FNV_START,
            buf: [0xFF; WRITE_CHUNK],
            buf_len: 0,
        })
    }

    pub fn append(&mut self, w: &mut Writer, mut data: &[u8]) -> Result<(), FsError> {
        if w.size() as usize + data.len() > MAX_FILE_SIZE as usize {
            return Err(FsError::TooLarge);
        }
        w.checksum = fnv1a(w.checksum, data);
        while !data.is_empty() {
            let n = (WRITE_CHUNK - w.buf_len).min(data.len());
            w.buf[w.buf_len..w.buf_len + n].copy_from_slice(&data[..n]);
            w.buf_len += n;
            data = &data[n..];
            if w.buf_len == WRITE_CHUNK {
                self.flush(w)?;
            }
        }
        Ok(())
    }

    /// Completes a write: stores size and checksum, commits, then marks
    /// older versions of the file dead.
    pub fn finish(&mut self, mut w: Writer) -> Result<(), FsError> {
        let total = w.size();
        // Pad the last chunk to a multiple of 4 bytes (the size field says
        // where the data really ends)
        let padded = w.buf_len.div_ceil(4) * 4;
        w.buf[w.buf_len..padded].fill(0xFF);
        w.buf_len = padded;
        self.flush(&mut w)?;

        let base = w.slot * SLOT_SIZE;
        let mut tail = [0u8; 8];
        tail[..4].copy_from_slice(&total.to_le_bytes());
        tail[4..].copy_from_slice(&w.checksum.to_le_bytes());
        self.flash.write(base + OFF_SIZE, &tail)?;
        self.flash.write(base + OFF_COMMIT, &[0; 4])?;

        let name = core::str::from_utf8(&w.name[..w.name_len as usize]).unwrap_or("");
        for slot in 0..self.slots {
            if slot == w.slot {
                continue;
            }
            if let Slot::File(info) = self.slot(slot)? {
                if info.name() == name && info.generation != w.generation {
                    self.kill(slot)?;
                }
            }
        }
        Ok(())
    }

    /// Deletes a file.
    pub fn remove(&mut self, name: &str) -> Result<(), FsError> {
        let mut found = false;
        for slot in 0..self.slots {
            if let Slot::File(info) = self.slot(slot)? {
                if info.name() == name {
                    self.kill(slot)?;
                    found = true;
                }
            }
        }
        if found {
            Ok(())
        } else {
            Err(FsError::NotFound)
        }
    }

    fn flush(&mut self, w: &mut Writer) -> Result<(), FsError> {
        if w.buf_len == 0 {
            return Ok(());
        }
        let start = w.slot * SLOT_SIZE + HEADER_SIZE + w.size;
        self.flash.write(start, &w.buf[..w.buf_len])?;
        w.size += w.buf_len as u32;
        w.buf_len = 0;
        Ok(())
    }

    /// Marks a slot dead by clearing its magic (no erase needed).
    fn kill(&mut self, slot: u32) -> Result<(), FsError> {
        self.flash.write(slot * SLOT_SIZE, &[0; 4])
    }

    /// The first slot not holding a committed file, starting at the cursor.
    fn free_slot(&mut self) -> Result<u32, FsError> {
        for i in 0..self.slots {
            let slot = (self.cursor + i) % self.slots;
            // A file being written looks unused, but its slot is taken
            if self.open.as_ref().is_some_and(|w| w.slot == slot) {
                continue;
            }
            if let Slot::Unused { .. } = self.slot(slot)? {
                return Ok(slot);
            }
        }
        Err(FsError::NoSpace)
    }

    fn slot(&mut self, slot: u32) -> Result<Slot, FsError> {
        let mut h = [0u8; HEADER_SIZE as usize];
        self.flash.read(slot * SLOT_SIZE, &mut h)?;
        let word =
            |off: u32| u32::from_le_bytes(h[off as usize..off as usize + 4].try_into().unwrap());
        let generation = word(OFF_GENERATION);
        if h[..4] != MAGIC {
            return Ok(Slot::Unused { generation: 0 });
        }
        let name_len = h[OFF_NAME_LEN as usize] as usize;
        let size = word(OFF_SIZE);
        let committed = word(OFF_COMMIT) == 0;
        if !committed || name_len == 0 || name_len > MAX_NAME_LEN || size > MAX_FILE_SIZE {
            // Unfinished write (or garbage): its generation still counts
            return Ok(Slot::Unused {
                generation: if generation == u32::MAX {
                    0
                } else {
                    generation
                },
            });
        }
        let mut name = [0u8; MAX_NAME_LEN];
        name[..name_len].copy_from_slice(&h[OFF_NAME as usize..OFF_NAME as usize + name_len]);
        Ok(Slot::File(FileInfo {
            slot,
            size,
            checksum: word(OFF_SIZE + 4),
            generation,
            name,
            name_len: name_len as u8,
        }))
    }
}

/// File names: 1-40 characters from `A-Z a-z 0-9 . _ -`, not `.` or `..`.
pub fn check_name(name: &str) -> Result<(), FsError> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(FsError::BadName)
    }
}

const FNV_START: u32 = 0x811C_9DC5;

fn fnv1a(hash: u32, data: &[u8]) -> u32 {
    data.iter()
        .fold(hash, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// What the interpreter needs from a file system (the `os` module and
/// `open()`): object safe, so the interpreter doesn't depend on the flash
/// type. One file can be open for writing at a time.
pub trait FileSystem {
    /// Calls `f(name, size)` for every file.
    fn list(&mut self, f: &mut dyn FnMut(&str, u32)) -> Result<(), FsError>;
    /// Size of a file, or `None` if it doesn't exist.
    fn file_size(&mut self, name: &str) -> Result<Option<u32>, FsError>;
    /// Reads from `offset`; returns the number of bytes read (0 at the end).
    fn read_at(&mut self, name: &str, offset: u32, buf: &mut [u8]) -> Result<usize, FsError>;
    /// Starts writing `name` (with `append`, after its current content).
    /// The new version replaces the old one at `close_file`. A file still
    /// open for writing is saved first. Returns a token for the file.
    fn create(&mut self, name: &str, append: bool) -> Result<u32, FsError>;
    fn write_file(&mut self, token: u32, data: &[u8]) -> Result<(), FsError>;
    /// Saves the file (a token that's no longer open is ignored).
    fn close_file(&mut self, token: u32) -> Result<(), FsError>;
    fn remove(&mut self, name: &str) -> Result<(), FsError>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError>;
    /// `(files, slots)`; every file takes one slot of `MAX_FILE_SIZE` bytes.
    fn usage(&mut self) -> Result<(u32, u32), FsError>;
}

impl<F: Flash> FileSystem for FlatFs<F> {
    fn list(&mut self, f: &mut dyn FnMut(&str, u32)) -> Result<(), FsError> {
        FlatFs::list(self, |info| f(info.name(), info.size))
    }

    fn file_size(&mut self, name: &str) -> Result<Option<u32>, FsError> {
        Ok(self.find(name)?.map(|info| info.size))
    }

    fn read_at(&mut self, name: &str, offset: u32, buf: &mut [u8]) -> Result<usize, FsError> {
        let file = self.find(name)?.ok_or(FsError::NotFound)?;
        FlatFs::read_at(self, &file, offset, buf)
    }

    fn create(&mut self, name: &str, append: bool) -> Result<u32, FsError> {
        if let Some(w) = self.open.take() {
            self.finish(w)?;
        }
        let mut w = self.begin(name)?;
        if append {
            // Copy the current content (still readable: it's replaced only at `finish`)
            if let Some(old) = self.find(name)? {
                let mut chunk = [0u8; WRITE_CHUNK];
                let mut at = 0;
                loop {
                    let n = FlatFs::read_at(self, &old, at, &mut chunk)?;
                    if n == 0 {
                        break;
                    }
                    self.append(&mut w, &chunk[..n])?;
                    at += n as u32;
                }
            }
        }
        self.open = Some(w);
        self.open_token = self.open_token.wrapping_add(1).max(1);
        Ok(self.open_token)
    }

    fn write_file(&mut self, token: u32, data: &[u8]) -> Result<(), FsError> {
        let mut w = match self.open.take() {
            Some(w) if token == self.open_token => w,
            other => {
                self.open = other;
                return Err(FsError::NotOpen);
            }
        };
        let result = self.append(&mut w, data);
        self.open = Some(w);
        result
    }

    fn close_file(&mut self, token: u32) -> Result<(), FsError> {
        if token != self.open_token {
            return Ok(());
        }
        match self.open.take() {
            Some(w) => self.finish(w),
            None => Ok(()),
        }
    }

    fn remove(&mut self, name: &str) -> Result<(), FsError> {
        FlatFs::remove(self, name)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let old = self.find(from)?.ok_or(FsError::NotFound)?;
        if from == to {
            return Ok(());
        }
        // A copy under the new name, then the old name is removed
        let mut w = self.begin(to)?;
        let mut chunk = [0u8; WRITE_CHUNK];
        let mut at = 0;
        loop {
            let n = FlatFs::read_at(self, &old, at, &mut chunk)?;
            if n == 0 {
                break;
            }
            self.append(&mut w, &chunk[..n])?;
            at += n as u32;
        }
        self.finish(w)?;
        FlatFs::remove(self, from)
    }

    fn usage(&mut self) -> Result<(u32, u32), FsError> {
        FlatFs::usage(self)
    }
}

/// Flash in RAM: for tests, and for running scripts with files on a PC.
pub struct RamFlash<'m> {
    mem: &'m mut [u8],
}

impl<'m> RamFlash<'m> {
    /// `mem.len()` should be a multiple of `SLOT_SIZE`; it's erased (0xFF).
    pub fn new(mem: &'m mut [u8]) -> Self {
        mem.fill(0xFF);
        Self { mem }
    }
}

impl Flash for RamFlash<'_> {
    fn size(&self) -> u32 {
        self.mem.len() as u32
    }

    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), FsError> {
        let o = offset as usize;
        buf.copy_from_slice(&self.mem[o..o + buf.len()]);
        Ok(())
    }

    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), FsError> {
        for (dst, src) in self.mem[offset as usize..].iter_mut().zip(data) {
            *dst &= src; // like NOR flash: bits only go from 1 to 0
        }
        Ok(())
    }

    fn erase(&mut self, offset: u32, len: u32) -> Result<(), FsError> {
        self.mem[offset as usize..(offset + len) as usize].fill(0xFF);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    /// RAM flash with NOR rules: writes can only clear bits, erases set whole
    /// sectors to 0xFF, offsets must be aligned. Can fail after N writes to
    /// simulate a power cut.
    struct RamFlash {
        mem: Vec<u8>,
        writes_left: Option<usize>,
        erases: usize,
    }

    impl RamFlash {
        fn new(slots: u32) -> Self {
            Self {
                mem: vec![0xFF; (slots * SLOT_SIZE) as usize],
                writes_left: None,
                erases: 0,
            }
        }
    }

    impl Flash for RamFlash {
        fn size(&self) -> u32 {
            self.mem.len() as u32
        }
        fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), FsError> {
            let o = offset as usize;
            buf.copy_from_slice(&self.mem[o..o + buf.len()]);
            Ok(())
        }
        fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), FsError> {
            assert!(
                offset.is_multiple_of(4) && data.len().is_multiple_of(4),
                "unaligned write"
            );
            if let Some(left) = &mut self.writes_left {
                if *left == 0 {
                    return Err(FsError::Io); // power cut
                }
                *left -= 1;
            }
            for (i, &b) in data.iter().enumerate() {
                let cell = &mut self.mem[offset as usize + i];
                assert_eq!(*cell & b, b, "NOR flash can't set bits without an erase");
                *cell &= b;
            }
            Ok(())
        }
        fn erase(&mut self, offset: u32, len: u32) -> Result<(), FsError> {
            assert!(
                offset.is_multiple_of(SECTOR_SIZE) && len.is_multiple_of(SECTOR_SIZE),
                "unaligned erase"
            );
            self.mem[offset as usize..(offset + len) as usize].fill(0xFF);
            self.erases += 1;
            Ok(())
        }
    }

    fn names(fs: &mut FlatFs<RamFlash>) -> Vec<(std::string::String, u32)> {
        let mut out = Vec::new();
        fs.list(|f| out.push((f.name().into(), f.size))).unwrap();
        out.sort();
        out
    }

    fn content(fs: &mut FlatFs<RamFlash>, name: &str) -> Vec<u8> {
        let mut buf = vec![0u8; MAX_FILE_SIZE as usize];
        let n = fs.read(name, &mut buf).unwrap();
        buf.truncate(n);
        buf
    }

    #[test]
    fn test_file_system_trait_streams_and_renames() {
        let mut mem = vec![0u8; (4 * SLOT_SIZE) as usize];
        let mut fs = FlatFs::mount(super::RamFlash::new(&mut mem)).unwrap();
        let fs: &mut dyn FileSystem = &mut fs;
        let t = fs.create("a.txt", false).unwrap();
        fs.write_file(t, b"hello ").unwrap();
        fs.write_file(t, b"world").unwrap();
        // Not visible until closed
        assert_eq!(fs.file_size("a.txt"), Ok(None));
        fs.close_file(t).unwrap();
        assert_eq!(fs.write_file(t, b"x"), Err(FsError::NotOpen));
        assert_eq!(fs.file_size("a.txt"), Ok(Some(11)));
        let mut buf = [0u8; 5];
        assert_eq!(fs.read_at("a.txt", 6, &mut buf), Ok(5));
        assert_eq!(&buf, b"world");

        // Appending; opening a second file saves the first
        let t1 = fs.create("a.txt", true).unwrap();
        fs.write_file(t1, b"!").unwrap();
        let t2 = fs.create("b.txt", false).unwrap();
        assert_eq!(fs.write_file(t1, b"?"), Err(FsError::NotOpen));
        fs.close_file(t1).unwrap(); // stale token: ignored
        fs.close_file(t2).unwrap();
        assert_eq!(fs.file_size("a.txt"), Ok(Some(12)));
        assert_eq!(fs.file_size("b.txt"), Ok(Some(0)));

        fs.rename("a.txt", "c.txt").unwrap();
        assert_eq!(fs.file_size("a.txt"), Ok(None));
        let mut names = std::vec::Vec::new();
        fs.list(&mut |name, size| names.push((std::string::String::from(name), size)))
            .unwrap();
        names.sort();
        assert_eq!(names, [("b.txt".into(), 0), ("c.txt".into(), 12)]);
        assert_eq!(fs.rename("nope", "x"), Err(FsError::NotFound));
        assert_eq!(fs.usage(), Ok((2, 4)));
    }

    #[test]
    fn test_write_read_list_remove() {
        let mut fs = FlatFs::mount(RamFlash::new(4)).unwrap();
        assert_eq!(fs.slot_count(), 4);
        assert!(names(&mut fs).is_empty());

        fs.write("main.py", b"print('hi')\n").unwrap();
        fs.write("lib.py", b"").unwrap();
        // Odd sizes exercise padding and chunk boundaries
        let big: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        fs.write("data.bin", &big).unwrap();

        assert_eq!(
            names(&mut fs),
            vec![
                ("data.bin".into(), 1000),
                ("lib.py".into(), 0),
                ("main.py".into(), 12)
            ]
        );
        assert_eq!(content(&mut fs, "main.py"), b"print('hi')\n");
        assert_eq!(content(&mut fs, "data.bin"), big);
        assert_eq!(content(&mut fs, "lib.py"), b"");
        assert_eq!(fs.usage().unwrap(), (3, 4));

        fs.remove("lib.py").unwrap();
        assert_eq!(fs.remove("lib.py"), Err(FsError::NotFound));
        assert_eq!(fs.find("lib.py").unwrap(), None);
        assert_eq!(fs.usage().unwrap(), (2, 4));

        // Survives a remount
        let mut fs = FlatFs::mount(fs.into_flash()).unwrap();
        assert_eq!(content(&mut fs, "data.bin"), big);
        assert_eq!(names(&mut fs).len(), 2);
    }

    #[test]
    fn test_replace_keeps_one_version_and_rotates_slots() {
        let mut fs = FlatFs::mount(RamFlash::new(3)).unwrap();
        let mut used_slots = Vec::new();
        for i in 0..6u8 {
            fs.write("main.py", &[b'a' + i; 10]).unwrap();
            used_slots.push(fs.find("main.py").unwrap().unwrap().slot);
            assert_eq!(names(&mut fs).len(), 1);
        }
        assert_eq!(content(&mut fs, "main.py"), [b'f'; 10]);
        // Each save goes to the next slot (wear leveling)
        assert_eq!(used_slots, vec![0, 1, 2, 0, 1, 2]);
    }

    #[test]
    fn test_full_and_limits() {
        let mut fs = FlatFs::mount(RamFlash::new(2)).unwrap();
        fs.write("a", b"1").unwrap();
        fs.write("b", b"2").unwrap();
        assert_eq!(fs.write("c", b"3"), Err(FsError::NoSpace));
        // Replacing needs a free slot for the new version too
        assert_eq!(fs.write("a", b"x"), Err(FsError::NoSpace));
        assert_eq!(content(&mut fs, "a"), b"1");

        let mut fs = FlatFs::mount(RamFlash::new(2)).unwrap();
        let max = vec![b'z'; MAX_FILE_SIZE as usize];
        fs.write("max", &max).unwrap();
        assert_eq!(content(&mut fs, "max"), max);
        let too_big = vec![0u8; MAX_FILE_SIZE as usize + 1];
        assert_eq!(fs.write("big", &too_big), Err(FsError::TooLarge));

        for bad in ["", ".", "..", "a/b", "with space", "ü.py", &"x".repeat(41)] {
            assert_eq!(fs.write(bad, b""), Err(FsError::BadName), "{:?}", bad);
        }
        assert_eq!(check_name(&"x".repeat(40)), Ok(()));
        assert_eq!(
            FlatFs::mount(RamFlash {
                mem: vec![0xFF; 4096],
                writes_left: None,
                erases: 0
            })
            .err(),
            Some(FsError::NoRegion)
        );
    }

    #[test]
    fn test_power_cut_during_replace_keeps_old_version() {
        // Cut the power at every possible write while replacing a file
        for cut_after in 0..12 {
            let mut fs = FlatFs::mount(RamFlash::new(3)).unwrap();
            fs.write("main.py", b"old version").unwrap();
            let mut flash = fs.into_flash();
            flash.writes_left = Some(cut_after);
            let mut fs = FlatFs::mount(flash).unwrap();
            let result = fs.write("main.py", &[b'n'; 600]);

            let mut flash = fs.into_flash();
            flash.writes_left = None;
            let mut fs = FlatFs::mount(flash).unwrap();
            let now = content(&mut fs, "main.py");
            match result {
                // Failed before the commit: the old version is still there
                Err(_) if now == b"old version" => {}
                // Committed (the cut hit the clean-up): the new version won
                _ => assert_eq!(now, [b'n'; 600], "cut after {} writes", cut_after),
            }
            assert_eq!(names(&mut fs).len(), 1, "cut after {} writes", cut_after);
            // The file system still works afterwards
            fs.write("other", b"ok").unwrap();
            assert_eq!(content(&mut fs, "other"), b"ok");
        }
    }

    #[test]
    fn test_duplicate_after_crash_is_cleaned_on_mount() {
        let mut fs = FlatFs::mount(RamFlash::new(3)).unwrap();
        fs.write("f", b"v1").unwrap();
        // A committed v2 whose clean-up never ran: two committed copies
        let w = fs.begin("f").unwrap();
        let mut flash = fs.into_flash();
        let base = (w.slot * SLOT_SIZE) as usize;
        let data = *b"v2\xFF\xFF";
        flash.mem[base + 64..base + 68].copy_from_slice(&data);
        let mut tail = [0u8; 8];
        tail[..4].copy_from_slice(&2u32.to_le_bytes());
        tail[4..].copy_from_slice(&fnv1a(FNV_START, b"v2").to_le_bytes());
        flash.mem[base + 52..base + 60].copy_from_slice(&tail);
        flash.mem[base + 60..base + 64].fill(0);

        let mut fs = FlatFs::mount(flash).unwrap();
        assert_eq!(content(&mut fs, "f"), b"v2");
        assert_eq!(names(&mut fs), vec![("f".into(), 2)]);
    }

    #[test]
    fn test_corruption_and_foreign_data() {
        let mut flash = RamFlash::new(3);
        // Leftover bytes from an old firmware image look like garbage
        flash.mem[..64].fill(0x5A);
        let mut fs = FlatFs::mount(flash).unwrap();
        assert!(names(&mut fs).is_empty());
        fs.write("a", b"hello").unwrap();

        // Flip a data bit: the checksum catches it
        let slot = fs.find("a").unwrap().unwrap().slot;
        let mut flash = fs.into_flash();
        flash.mem[(slot * SLOT_SIZE + 64) as usize] ^= 1;
        let mut fs = FlatFs::mount(flash).unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(fs.read("a", &mut buf), Err(FsError::Corrupt));
        assert_eq!(fs.read("zzz", &mut buf), Err(FsError::NotFound));
        assert_eq!(fs.read("a", &mut buf[..2]), Err(FsError::TooLarge));
    }

    #[test]
    fn test_streaming_writer_and_partial_reads() {
        let mut fs = FlatFs::mount(RamFlash::new(2)).unwrap();
        let mut w = fs.begin("log.txt").unwrap();
        let mut expected = Vec::new();
        for i in 0..100u32 {
            let line = std::format!("line {}\n", i);
            fs.append(&mut w, line.as_bytes()).unwrap();
            expected.extend_from_slice(line.as_bytes());
        }
        assert_eq!(w.size() as usize, expected.len());
        fs.finish(w).unwrap();

        let file = fs.find("log.txt").unwrap().unwrap();
        let mut got = Vec::new();
        let mut chunk = [0u8; 100];
        let mut offset = 0;
        loop {
            let n = fs.read_at(&file, offset, &mut chunk).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&chunk[..n]);
            offset += n as u32;
        }
        assert_eq!(got, expected);
        assert!(FsError::NoSpace.message().starts_with("no free file slot"));
    }
}
