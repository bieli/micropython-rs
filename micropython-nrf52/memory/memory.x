/* nRF52840 with the Adafruit nRF52 UF2 bootloader and SoftDevice S140 6.1.1
 * (nice!nano v2), as the bootloader lays out the 1 MB of flash:
 *
 *   0x00000-0x01000  MBR (Nordic's master boot record)
 *   0x01000-0x26000  SoftDevice S140 6.1.1 (Bluetooth; not used, kept: the
 *                    bootloader and the MBR expect it, and it forwards the
 *                    interrupts to the application while it's disabled)
 *   0x26000-0xA6000  this firmware (512 KB)
 *   0xA6000-0xF4000  the file system (312 KB; storage.rs)
 *   0xF4000-0x100000 the bootloader and its settings (never written)
 *
 * RAM: all 256 KB (the SoftDevice only uses RAM once it's enabled), but the
 * first 8 bytes, where the MBR keeps the address it forwards interrupts to.
 */
MEMORY
{
  FLASH : ORIGIN = 0x00026000, LENGTH = 512K
  RAM   : ORIGIN = 0x20000008, LENGTH = 256K - 8
}

_vfs_start = 0x000A6000;
_vfs_size  = 0x000F4000 - 0x000A6000;
ASSERT(ORIGIN(FLASH) + LENGTH(FLASH) <= _vfs_start, "the firmware overlaps the file system");
