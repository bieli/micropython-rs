#!/usr/bin/env python3
"""Flash and RAM use of the XIAO MG24 firmware, from its ELF section headers
(no binutils needed): code, constants, the data copied to RAM at boot, the
statics, and what's left for the stack."""
import struct
import sys

# The firmware's part of the flash (memory/mg24.x; the file system has the rest)
FLASH, RAM = 576 * 1024, 256 * 1024


def sections(path):
    data = open(path, "rb").read()
    assert data[:4] == b"\x7fELF" and data[4] == 1, "a 32-bit ELF"
    shoff, = struct.unpack_from("<I", data, 0x20)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", data, 0x2E)
    heads = [struct.unpack_from("<IIIIIIIIII", data, shoff + i * shentsize) for i in range(shnum)]
    names = heads[shstrndx][4]
    for name, kind, flags, addr, offset, size, *_ in heads:
        end = data.index(b"\0", names + name)
        yield data[names + name:end].decode(), kind, flags, addr, size


def main(path):
    sizes = {}
    for name, kind, flags, addr, size in sections(path):
        if flags & 2 and size:  # SHF_ALLOC
            sizes[name] = (addr, size)
    get = lambda n: sizes.get(n, (0, 0))[1]
    code = get(".vector_table") + get(".text")
    rodata = get(".rodata")
    data = get(".data") + get(".ram_text")
    statics = data + get(".bss") + get(".uninit")
    flash = code + rodata + data
    print(f"Firmware memory: {path}")
    print(f"  code         {code:8d} bytes {code / 1024:7.1f} KB")
    print(f"  constants    {rodata:8d} bytes {rodata / 1024:7.1f} KB")
    print(f"  flash total  {flash:8d} bytes {flash / 1024:7.1f} KB of {FLASH // 1024} KB")
    print(f"  RAM statics  {statics:8d} bytes {statics / 1024:7.1f} KB")
    print(f"  stack        {RAM - statics:8d} bytes {(RAM - statics) / 1024:7.1f} KB (the rest of {RAM // 1024} KB)")


if __name__ == "__main__":
    main(sys.argv[1])
