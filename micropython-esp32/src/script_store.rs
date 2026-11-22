//! `main.py` saved in flash, so a script can be replaced over USB serial
//! without rebuilding the firmware.
//!
//! The script lives in the `nvs` partition (offset 0x9000, 24 KB), which
//! every espflash partition table has and this firmware doesn't otherwise
//! use. Layout: `b"MPY1"`, length (u32 LE), FNV-1a checksum (u32 LE), text.
//! `make flash` erases the partition (`--erase-parts nvs`) so a fresh build
//! runs its embedded script, unless `KEEP_MAIN=1` is given.

use embedded_storage::{ReadStorage, Storage};
use esp_hal::peripherals::FLASH;
use esp_storage::FlashStorage;

const REGION_OFFSET: u32 = 0x9000;
const REGION_SIZE: usize = 0x6000;
const MAGIC: [u8; 4] = *b"MPY1";
const HEADER_SIZE: usize = 12;

/// Largest script that fits.
pub const MAX_SCRIPT: usize = REGION_SIZE - HEADER_SIZE;

pub struct ScriptStore<'d> {
    flash: FlashStorage<'d>,
}

#[derive(Debug)]
pub enum StoreError {
    TooLarge,
    Flash,
}

impl<'d> ScriptStore<'d> {
    pub fn new(flash: FLASH<'d>) -> Self {
        Self {
            flash: FlashStorage::new(flash),
        }
    }

    /// Reads the saved script into `dst`, returning its length, or `None` if
    /// nothing valid is saved.
    pub fn load(&mut self, dst: &mut [u8]) -> Option<usize> {
        let mut header = [0u8; HEADER_SIZE];
        self.flash.read(REGION_OFFSET, &mut header).ok()?;
        if header[..4] != MAGIC {
            return None;
        }
        let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        let checksum = u32::from_le_bytes(header[8..12].try_into().unwrap());
        if len > MAX_SCRIPT || len > dst.len() {
            return None;
        }
        self.flash
            .read(REGION_OFFSET + HEADER_SIZE as u32, &mut dst[..len])
            .ok()?;
        (fnv1a(&dst[..len]) == checksum).then_some(len)
    }

    /// Saves `script` as `main.py`. The header is written last, so an
    /// interrupted save leaves no valid script rather than a broken one.
    pub fn save(&mut self, script: &[u8]) -> Result<(), StoreError> {
        if script.len() > MAX_SCRIPT {
            return Err(StoreError::TooLarge);
        }
        self.erase()?;
        self.flash
            .write(REGION_OFFSET + HEADER_SIZE as u32, script)
            .map_err(|_| StoreError::Flash)?;

        let mut header = [0u8; HEADER_SIZE];
        header[..4].copy_from_slice(&MAGIC);
        header[4..8].copy_from_slice(&(script.len() as u32).to_le_bytes());
        header[8..12].copy_from_slice(&fnv1a(script).to_le_bytes());
        self.flash
            .write(REGION_OFFSET, &header)
            .map_err(|_| StoreError::Flash)
    }

    /// Removes the saved script (the embedded one runs at boot again).
    pub fn erase(&mut self) -> Result<(), StoreError> {
        self.flash
            .write(REGION_OFFSET, &[0xFF; HEADER_SIZE])
            .map_err(|_| StoreError::Flash)
    }
}

fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811C_9DC5u32, |hash, &byte| {
        (hash ^ byte as u32).wrapping_mul(0x0100_0193)
    })
}
