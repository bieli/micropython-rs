//! Files in the chip's own flash: the core library's flat file system
//! (`micropython_rs::flatfs`, as on the ESP32 boards) on the 960 KB VFS
//! region the linker script keeps above the firmware (memory/mg24.x).
//!
//! The flash is memory-mapped, so reads are plain loads. Writes and erases go
//! through the MSC (the memory system controller): unlocked, one 32-bit word
//! at a time, a page (8 KB) at a time, waiting for each to finish. The flash
//! can't be read meanwhile (an erase takes about 20 ms): a fetch from it
//! stalls the whole core, interrupts included. So the loops that start the
//! operations and wait for them run from RAM (`.ram_text`), as do the
//! console's and the timer's interrupt handlers and the vector table
//! (main.rs): the console keeps receiving while a file is saved.
//! The instruction cache, which also serves data reads from flash, is
//! flushed after every change. Every access is checked against the region:
//! nothing outside it is ever written.

use micropython_rs::flatfs::{Flash, FlatFs, FsError, SECTOR_SIZE};

use efr32mg24_pac as pac;

/// MSC's flash page: the erase unit.
const PAGE_SIZE: u32 = 8192;
/// MSC_LOCK's unlock key.
const UNLOCK: u16 = 7025;

extern "C" {
    // From memory/mg24.x
    static _vfs_start: u8;
    static _vfs_size: u8;
}

pub type Files = FlatFs<MscFlash>;

/// The VFS region of the internal flash.
pub struct MscFlash {
    base: u32,
    size: u32,
}

fn msc() -> &'static pac::msc_s::RegisterBlock {
    // SAFETY: MSC's registers; only this module uses them
    unsafe { &*pac::MscS::ptr() }
}

impl MscFlash {
    fn region(&self, offset: u32, len: u32) -> Result<u32, FsError> {
        match offset.checked_add(len) {
            Some(end) if end <= self.size => Ok(self.base + offset),
            _ => Err(FsError::Io),
        }
    }

    /// Runs `op` with the MSC unlocked and writes enabled, then locks it and
    /// flushes the cache.
    fn unlocked(
        op: impl FnOnce(&pac::msc_s::RegisterBlock) -> Result<(), FsError>,
    ) -> Result<(), FsError> {
        let msc = msc();
        msc.lock().write(|w| unsafe { w.lockkey().bits(UNLOCK) });
        msc.writectrl().modify(|_, w| w.wren().set_bit());
        let result = op(msc);
        msc.writectrl().modify(|_, w| w.wren().clear_bit());
        msc.lock().write(|w| unsafe { w.lockkey().bits(0) });
        // SAFETY: ICACHE0's command register: a flush only drops cached lines
        unsafe {
            (&*pac::Icache0S::ptr())
                .cmd()
                .write(|w| w.flush().set_bit())
        };
        result
    }
}

// MSC_STATUS and MSC_WRITECMD bits
const BUSY: u32 = 1 << 0;
const LOCKED: u32 = 1 << 1;
const INVADDR: u32 = 1 << 2;
const WDATAREADY: u32 = 1 << 3;
const CMD_ERASEPAGE: u32 = 1 << 1;
const CMD_WRITEEND: u32 = 1 << 2;

/// The MSC registers the RAM routines use, as plain pointers (the PAC's code
/// is in flash).
struct Regs {
    addrb: *mut u32,
    wdata: *mut u32,
    writecmd: *mut u32,
    status: *const u32,
}

impl Regs {
    fn new(msc: &pac::msc_s::RegisterBlock) -> Self {
        Self {
            addrb: msc.addrb() as *const _ as *mut u32,
            wdata: msc.wdata() as *const _ as *mut u32,
            writecmd: msc.writecmd() as *const _ as *mut u32,
            status: msc.status() as *const _ as *const u32,
        }
    }
}

/// Erases the page at `page`; returns MSC_STATUS's error bits. Runs from
/// RAM (and calls nothing): while the page is erased, the flash can't be read.
///
/// # Safety
/// `r` are the MSC's registers, unlocked, writes enabled.
#[inline(never)]
#[link_section = ".ram_text.msc_erase"]
unsafe fn ram_erase(r: &Regs, page: u32) -> u32 {
    unsafe {
        r.addrb.write_volatile(page);
        r.writecmd.write_volatile(CMD_ERASEPAGE);
        while r.status.read_volatile() & BUSY != 0 {}
        r.status.read_volatile() & (LOCKED | INVADDR)
    }
}

/// Programs `n` words from `src` (any alignment) at `at`, skipping erased
/// ones (0xFFFFFFFF); returns MSC_STATUS's error bits. Runs from RAM.
///
/// # Safety
/// As `ram_erase`; `src` holds `4 * n` readable bytes in RAM.
#[inline(never)]
#[link_section = ".ram_text.msc_write"]
unsafe fn ram_write(r: &Regs, at: u32, src: *const u8, n: usize) -> u32 {
    unsafe {
        let mut errors = 0;
        let mut i = 0;
        while i < n {
            let b = src.add(4 * i);
            let word = b.read() as u32
                | (b.add(1).read() as u32) << 8
                | (b.add(2).read() as u32) << 16
                | (b.add(3).read() as u32) << 24;
            if word != u32::MAX {
                r.addrb.write_volatile(at + 4 * i as u32);
                while r.status.read_volatile() & WDATAREADY == 0 {}
                r.wdata.write_volatile(word);
                while r.status.read_volatile() & BUSY != 0 {}
                errors |= r.status.read_volatile() & (LOCKED | INVADDR);
            }
            i += 1;
        }
        r.writecmd.write_volatile(CMD_WRITEEND);
        while r.status.read_volatile() & BUSY != 0 {}
        errors | r.status.read_volatile() & (LOCKED | INVADDR)
    }
}

fn check(errors: u32) -> Result<(), FsError> {
    if errors == 0 {
        Ok(())
    } else {
        Err(FsError::Io)
    }
}

impl Flash for MscFlash {
    fn size(&self) -> u32 {
        self.size
    }

    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), FsError> {
        let at = self.region(offset, buf.len() as u32)?;
        // SAFETY: inside the VFS region of the memory-mapped flash
        unsafe { core::ptr::copy_nonoverlapping(at as *const u8, buf.as_mut_ptr(), buf.len()) };
        Ok(())
    }

    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), FsError> {
        if !offset.is_multiple_of(4) || !data.len().is_multiple_of(4) {
            return Err(FsError::Io);
        }
        let at = self.region(offset, data.len() as u32)?;
        // SAFETY: the MSC unlocked by `unlocked`; `data` is in RAM (flatfs
        // writes from buffers on the stack or in the REPL's memory)
        Self::unlocked(|msc| {
            check(unsafe { ram_write(&Regs::new(msc), at, data.as_ptr(), data.len() / 4) })
        })
    }

    fn erase(&mut self, offset: u32, len: u32) -> Result<(), FsError> {
        if !offset.is_multiple_of(PAGE_SIZE) || !len.is_multiple_of(PAGE_SIZE) {
            return Err(FsError::Io); // flatfs erases whole 32 KB slots
        }
        let at = self.region(offset, len)?;
        Self::unlocked(|msc| {
            let regs = Regs::new(msc);
            for page in (at..at + len).step_by(PAGE_SIZE as usize) {
                // SAFETY: the MSC unlocked by `unlocked`; a page of the region
                check(unsafe { ram_erase(&regs, page) })?;
            }
            Ok(())
        })
    }
}

// flatfs's slots are whole pages (its 4 KB sectors, 8 to a slot, are 4 pages)
const _: () = assert!((8 * SECTOR_SIZE).is_multiple_of(PAGE_SIZE));

/// Mounts the file system on the VFS region, or `None` with a message on
/// the console.
pub fn mount() -> Option<Files> {
    // SAFETY: the bus clocks of MSC and ICACHE0 (CMU_CLKEN1), set once at
    // boot: an access to their registers without them is a bus fault
    unsafe {
        (&*pac::CmuS::ptr())
            .clken1()
            .modify(|_, w| w.msc().set_bit().icache0().set_bit())
    };
    // Only the linker symbols' addresses are taken
    let base = core::ptr::addr_of!(_vfs_start) as u32;
    let size = core::ptr::addr_of!(_vfs_size) as u32;
    match FlatFs::mount(MscFlash { base, size }) {
        Ok(files) => Some(files),
        Err(err) => {
            crate::console::write_crlf("File system error: ");
            crate::console::write_crlf(err.message());
            crate::console::write_crlf("\n");
            None
        }
    }
}
