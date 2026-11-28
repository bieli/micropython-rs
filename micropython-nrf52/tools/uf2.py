#!/usr/bin/env python3
"""ELF -> UF2 for the Adafruit nRF52 bootloader (no other tools needed).

    python3 tools/uf2.py firmware.elf firmware.uf2

The UF2 file is what the bootloader's USB drive takes: 512-byte blocks of
256 bytes of flash each, with the nRF52840's family ID (0xADA52840). The
data is the ELF's allocated sections at their load addresses (the initial
values of .data sit in flash after the code)."""
import struct
import sys

FAMILY_NRF52840 = 0xADA52840
FLAG_FAMILY = 0x00002000
MAGIC0, MAGIC1, MAGIC_END = 0x0A324655, 0x9E5D5157, 0x0AB16F30
PAYLOAD = 256


def segments(path):
    """(load address, bytes) of each section in the image, as objcopy
    -O binary takes them: allocated sections with contents (not .bss), at
    their load address (their file offset within the segment that loads
    them). A segment's bytes outside its sections, such as the ELF headers
    the linker may load, are left out."""
    data = open(path, "rb").read()
    assert data[:4] == b"\x7fELF" and data[4] == 1 and data[5] == 1, "a 32-bit little-endian ELF"
    phoff, shoff = struct.unpack_from("<II", data, 0x1C)
    phentsize, phnum, shentsize, shnum = struct.unpack_from("<HHHH", data, 0x2A)
    loads = []
    for i in range(phnum):
        kind, offset, _vaddr, paddr, filesz = struct.unpack_from("<5I", data, phoff + i * phentsize)
        if kind == 1:  # PT_LOAD
            loads.append((offset, filesz, paddr))
    for i in range(shnum):
        _name, kind, flags, _addr, offset, size = struct.unpack_from("<6I", data, shoff + i * shentsize)
        if kind != 1 or not flags & 2 or size == 0:  # PROGBITS, SHF_ALLOC
            continue
        seg = [l for l in loads if l[0] <= offset and offset + size <= l[0] + l[1]]
        assert seg, "a section outside every loadable segment"
        seg_offset, _, paddr = seg[0]
        yield paddr + offset - seg_offset, data[offset:offset + size]


def image(path):
    """{256-byte-aligned address: 256 bytes}, gaps filled with 0xFF."""
    pages = {}
    for addr, blob in segments(path):
        for i, byte in enumerate(blob):
            a = addr + i
            page = pages.setdefault(a - a % PAYLOAD, bytearray(b"\xff" * PAYLOAD))
            page[a % PAYLOAD] = byte
    return pages


def uf2(pages):
    out = bytearray()
    for n, addr in enumerate(sorted(pages)):
        block = struct.pack("<8I", MAGIC0, MAGIC1, FLAG_FAMILY, addr, PAYLOAD, n, len(pages),
                            FAMILY_NRF52840)
        block += pages[addr] + b"\0" * (476 - PAYLOAD) + struct.pack("<I", MAGIC_END)
        assert len(block) == 512
        out += block
    return bytes(out)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    pages = image(sys.argv[1])
    lo, hi = min(pages), max(pages) + PAYLOAD
    if lo < 0x26000 or hi > 0xF4000:
        sys.exit("uf2.py: %#x-%#x is outside the application's flash (0x26000-0xF4000)" % (lo, hi))
    open(sys.argv[2], "wb").write(uf2(pages))
    print("%s: %d blocks, %#x-%#x (%.1f KB)" % (sys.argv[2], len(pages), lo, hi, (hi - lo) / 1024))
