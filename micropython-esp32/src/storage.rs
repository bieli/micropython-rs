//! Files in flash: the core library's flat file system (`micropython_rs::flatfs`)
//! on the `vfs` partition (see `partitions.csv`), found by its label in the
//! partition table at boot.
//!
//! Older firmware kept a single `main.py` in the `nvs` partition; at the first
//! boot with this firmware it's moved into the file system.

use embedded_storage::nor_flash::NorFlash;
use embedded_storage::{ReadStorage, Storage};
use esp_bootloader_esp_idf::partitions::{read_partition_table, PARTITION_TABLE_MAX_LEN};
use esp_hal::peripherals::FLASH;
use esp_println::println;
use esp_storage::FlashStorage;
use micropython_rs::flatfs::{Flash, FlatFs, FsError};

/// Label of the file system partition in `partitions.csv`.
pub const VFS_LABEL: &str = "vfs";

/// The flash region of the `vfs` partition.
pub struct VfsFlash {
    storage: FlashStorage<'static>,
    offset: u32,
    size: u32,
}

pub type Files = FlatFs<VfsFlash>;

impl Flash for VfsFlash {
    fn size(&self) -> u32 {
        self.size
    }

    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), FsError> {
        // `ReadStorage` accepts any alignment (NorFlash reads need 4 bytes)
        ReadStorage::read(&mut self.storage, self.offset + offset, buf).map_err(|_| FsError::Io)
    }

    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), FsError> {
        // NorFlash writes only clear bits, without a hidden read-modify-write
        NorFlash::write(&mut self.storage, self.offset + offset, data).map_err(|_| FsError::Io)
    }

    fn erase(&mut self, offset: u32, len: u32) -> Result<(), FsError> {
        let from = self.offset + offset;
        NorFlash::erase(&mut self.storage, from, from + len).map_err(|_| FsError::Io)
    }
}

/// The flash chip's size in bytes, from the bootloader's image header (set
/// by `mount`; `lscpu` reports it).
static FLASH_SIZE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The flash chip's size in bytes (0 before `mount`).
pub fn flash_size() -> u32 {
    FLASH_SIZE.load(core::sync::atomic::Ordering::Relaxed)
}

/// Mounts the file system, or returns `None` (with a message) if the flash
/// has no `vfs` partition, e.g. when flashed without `partitions.csv`.
#[inline(never)]
pub fn mount(flash: FLASH<'static>) -> Option<Files> {
    let mut storage = FlashStorage::new(flash);
    FLASH_SIZE.store(
        storage.capacity() as u32,
        core::sync::atomic::Ordering::Relaxed,
    );

    let mut table_buf = [0u8; PARTITION_TABLE_MAX_LEN];
    let region = read_partition_table(&mut storage, &mut table_buf)
        .ok()
        .and_then(|table| {
            table
                .iter()
                .find(|p| p.label_as_str() == VFS_LABEL)
                .map(|p| (p.offset(), p.len()))
        });
    let Some((offset, size)) = region else {
        println!(
            "No '{}' partition: files are disabled. Flash with `make flash` (uses partitions.csv).",
            VFS_LABEL
        );
        return None;
    };

    let mut files = match FlatFs::mount(VfsFlash {
        storage,
        offset,
        size,
    }) {
        Ok(files) => files,
        Err(err) => {
            println!("File system error: {}", err.message());
            return None;
        }
    };
    migrate_legacy_main(&mut files);
    Some(files)
}

// --- main.py from older firmware (nvs partition, 0x9000) ----------------------

const LEGACY_OFFSET: u32 = 0x9000;
const LEGACY_MAGIC: [u8; 4] = *b"MPY1";
const LEGACY_HEADER: usize = 12;
const LEGACY_MAX: usize = 0x6000 - LEGACY_HEADER;

/// Moves a `main.py` saved by older firmware into the file system (unless the
/// file system already has one), then clears the old copy.
#[inline(never)]
fn migrate_legacy_main(files: &mut Files) {
    let flash = &mut files.flash_mut().storage;
    let mut header = [0u8; LEGACY_HEADER];
    if ReadStorage::read(flash, LEGACY_OFFSET, &mut header).is_err() || header[..4] != LEGACY_MAGIC
    {
        return;
    }
    let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    let checksum = u32::from_le_bytes(header[8..12].try_into().unwrap());
    if len > LEGACY_MAX {
        return;
    }

    let mut script = [0u8; LEGACY_MAX];
    let ok = ReadStorage::read(
        flash,
        LEGACY_OFFSET + LEGACY_HEADER as u32,
        &mut script[..len],
    )
    .is_ok()
        && fnv1a(&script[..len]) == checksum;
    // The old header is cleared either way, so this runs only once
    let _ = Storage::write(flash, LEGACY_OFFSET, &[0xFF; LEGACY_HEADER]);
    if !ok {
        return;
    }

    if matches!(files.find("main.py"), Ok(None)) {
        match files.write("main.py", &script[..len]) {
            Ok(()) => println!(
                "Moved main.py ({} bytes) from the old storage into the file system",
                len
            ),
            Err(err) => println!("Could not move the old main.py: {}", err.message()),
        }
    }
}

fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811C_9DC5u32, |hash, &byte| {
        (hash ^ byte as u32).wrapping_mul(0x0100_0193)
    })
}
