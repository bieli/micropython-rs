#!/usr/bin/env python3
"""Memory report of an ESP32 firmware ELF, with limits for CI.

    tools/fw_report.py target/xtensa-esp32-none-elf/release/micropython-esp32
    tools/fw_report.py ELF --min-stack-kb 128 --max-frame-kb 16 --markdown >> $GITHUB_STEP_SUMMARY

Prints the size of each memory region (flash code and data, IRAM, DRAM,
dram2), the main stack (what the statics leave of the main data RAM) and the
biggest stack frames. The frames come from the Xtensa `entry a1, N`
instruction every function starts with (N bytes of stack for the call), so a
local array that silently grows a frame shows up here.

Exits with 1 when the stack is smaller than --min-stack-kb, a frame is bigger
than --max-frame-kb, or the app doesn't fit its partition (--app-kb). Needs
only Python 3; `c++filt` (binutils), if found, makes function names readable.
"""

import argparse
import shutil
import struct
import subprocess
import sys

SHT_SYMTAB, SHT_NOBITS = 2, 8
STT_FUNC = 2


def read_elf(path):
    data = open(path, 'rb').read()
    if data[:4] != b'\x7fELF' or data[4] != 1 or data[5] != 1:
        sys.exit(f'{path}: not a 32-bit little-endian ELF file')
    shoff, = struct.unpack_from('<I', data, 0x20)
    shentsize, shnum, shstrndx = struct.unpack_from('<HHH', data, 0x2e)
    sections = []
    for i in range(shnum):
        name, typ, flags, addr, off, size, link, info, align, entsize = \
            struct.unpack_from('<10I', data, shoff + i * shentsize)
        sections.append(dict(name_off=name, type=typ, flags=flags, addr=addr,
                             off=off, size=size, link=link, entsize=entsize))
    names = sections[shstrndx]
    for s in sections:
        start = names['off'] + s['name_off']
        s['name'] = data[start:data.index(b'\0', start)].decode()
    symbols = {}
    functions = []
    for s in sections:
        if s['type'] != SHT_SYMTAB:
            continue
        strtab = sections[s['link']]
        for i in range(s['size'] // 16):
            name, value, size, info, other, shndx = \
                struct.unpack_from('<IIIBBH', data, s['off'] + i * 16)
            start = strtab['off'] + name
            text = data[start:data.index(b'\0', start)].decode(errors='replace')
            symbols[text] = value
            if info & 0xf == STT_FUNC and size:
                functions.append((text, value, size))
    return data, sections, symbols, functions


def read_at(data, sections, address, n):
    """`n` bytes of the loaded image at `address`, or None."""
    for s in sections:
        if s['type'] != SHT_NOBITS and s['size'] and s['addr'] <= address and address + n <= s['addr'] + s['size']:
            off = s['off'] + address - s['addr']
            return data[off:off + n]
    return None


def frame_size(data, sections, address):
    """Bytes of stack the function at `address` takes.

    Its first instruction is `entry a1, N`: N bytes (at most 32 760). A bigger
    frame starts with a small `entry`, loads its size from a literal (`l32r
    aX, ...`), subtracts it and moves the stack pointer (`movsp a1, aY`):
    that's looked for in the next instructions."""
    code = read_at(data, sections, address, 64)
    if not code:
        return None
    b0, b1, b2 = code[:3]
    # entry as, imm12: op0 = 6, n = 3, m = 0 (0x36); as = a1, the stack pointer
    if b0 != 0x36 or b1 & 0x0f != 1:
        return None
    size = ((b1 >> 4) | (b2 << 4)) * 8
    literal, i = None, 3
    while i + 3 <= len(code):
        op0 = code[i] & 0x0f
        if 8 <= op0 <= 0x0d:          # 16-bit (narrow) instruction
            i += 2
            continue
        word = code[i] | code[i + 1] << 8 | code[i + 2] << 16
        if op0 == 1 and literal is None:    # l32r at, label (the label is before the code)
            pc = address + i
            target = ((pc + 3) & ~3) + ((word >> 8) - 0x10000) * 4   # a negative word offset
            raw = read_at(data, sections, target, 4)
            if raw:
                literal = struct.unpack('<I', raw)[0]
        elif word & 0xfff0ff == 0x001010:   # movsp a1, as
            if literal is not None and literal < 1 << 24:
                return size + literal
            return size
        i += 3
    return size


def v0_names(symbol):
    """The identifiers in a Rust v0 symbol (`_RNvNtCs..._5crate6module4func`
    gives `crate::module::func`): enough to recognize a function when no
    demangler knows the format (binutils before 2.36)."""
    parts, i = [], 2
    while i < len(symbol):
        c = symbol[i]
        if c in 'sB':   # disambiguator (crate hash) s<base62>_, back-reference B<base62>_
            i = symbol.find('_', i) + 1 or len(symbol)
        elif c.isdigit():
            j = i
            while j < len(symbol) and symbol[j].isdigit():
                j += 1
            n = int(symbol[i:j])
            if j < len(symbol) and symbol[j] == '_':
                j += 1
            parts.append(symbol[j:j + n])
            i = j + n
        else:
            i += 1
    if len(parts) > 4:   # generic arguments in between: keep the path and the function
        parts = parts[:3] + ['…'] + parts[-1:]
    return '::'.join(parts) or symbol


def demangle(names):
    tool = shutil.which('c++filt')
    out = names
    if tool and names:
        try:
            out = subprocess.run([tool], input='\n'.join(names), capture_output=True,
                                 text=True, check=True).stdout.splitlines()
        except (OSError, subprocess.CalledProcessError):
            pass
    # Drop the `::h0123...` hash of the legacy mangling
    out = [n.rsplit('::h', 1)[0] if len(n.rsplit('::h', 1)[-1]) == 16 else n for n in out]
    return [v0_names(n) if n.startswith('_R') else n for n in out]


def region(addr):
    # ESP32-S3 (its address ranges don't overlap the ESP32's): flash code and
    # data through the cache, IRAM and DRAM views of the same internal SRAM
    if 0x42000000 <= addr < 0x44000000:
        return 'flash code'
    if 0x3c000000 <= addr < 0x3e000000:
        return 'flash data'
    if 0x40370000 <= addr < 0x403e0000:
        return 'IRAM'
    if 0x3fcdb700 <= addr < 0x3fced710:
        return 'dram2'
    if 0x3fc88000 <= addr < 0x3fd00000:
        return 'DRAM'
    # ESP32
    if 0x400d0000 <= addr < 0x40400000:
        return 'flash code'
    if 0x3f400000 <= addr < 0x3f800000:
        return 'flash data'
    if 0x40070000 <= addr < 0x400a0000:
        return 'IRAM'
    if 0x3ffe7e30 <= addr < 0x40000000:
        return 'dram2'
    if 0x3ff80000 <= addr < 0x3ffe7e30:
        return 'DRAM'
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('elf')
    ap.add_argument('--min-stack-kb', type=int, default=0, help='fail below this main stack size')
    ap.add_argument('--max-frame-kb', type=int, default=0, help='fail when a function takes more stack')
    ap.add_argument('--app-kb', type=int, default=3072, help='size of the app partition (3 MB)')
    ap.add_argument('--frames', type=int, default=10, help='how many of the biggest frames to list')
    ap.add_argument('--markdown', action='store_true', help='GitHub step summary format')
    args = ap.parse_args()

    data, sections, symbols, functions = read_elf(args.elf)
    regions = {}
    flash_image = 0
    for s in sections:
        if s['size'] == 0 or s['name'] in ('.stack',) or not s['addr']:
            continue
        r = region(s['addr'])
        if r:
            regions[r] = regions.get(r, 0) + s['size']
            if s['type'] != SHT_NOBITS:
                flash_image += s['size']   # every loaded section is stored in flash
    stack = symbols.get('_stack_start_cpu0', 0) - symbols.get('_stack_end_cpu0', 0)

    frames = []
    for name, addr, size in functions:
        n = frame_size(data, sections, addr & ~1)
        if n:
            frames.append((n, name))
    frames.sort(reverse=True)
    top = frames[:args.frames]
    names = demangle([name for _, name in top])

    problems = []
    if args.min_stack_kb and stack < args.min_stack_kb * 1024:
        problems.append(f'main stack {stack // 1024} KB < {args.min_stack_kb} KB')
    if args.max_frame_kb and top and top[0][0] > args.max_frame_kb * 1024:
        problems.append(f'{names[0]} takes {top[0][0]} bytes of stack > {args.max_frame_kb} KB')
    if flash_image > args.app_kb * 1024:
        problems.append(f'app {flash_image // 1024} KB > partition {args.app_kb} KB')

    md = args.markdown
    title = args.elf.rsplit('/', 1)[-1]
    print(f'### Firmware memory: `{title}`\n' if md else f'Firmware memory: {args.elf}')
    rows = [(r, regions.get(r, 0)) for r in ('flash code', 'flash data', 'IRAM', 'DRAM', 'dram2')]
    rows.append(('main stack', stack))
    rows.append(('app image', flash_image))
    if md:
        print('| Region | Bytes | KB |\n|---|---:|---:|')
        for r, n in rows:
            print(f'| {r} | {n} | {n / 1024:.1f} |')
        print(f'\nBiggest stack frames:\n\n| Bytes | Function |\n|---:|---|')
        for (n, _), name in zip(top, names):
            print(f'| {n} | `{name[:120]}` |')
    else:
        for r, n in rows:
            print(f'  {r:<11} {n:>8} bytes  {n / 1024:7.1f} KB')
        print('Biggest stack frames:')
        for (n, _), name in zip(top, names):
            print(f'  {n:>6}  {name[:110]}')
    for p in problems:
        print(f'\n**FAIL:** {p}' if md else f'FAIL: {p}')
    sys.exit(1 if problems else 0)


if __name__ == '__main__':
    main()
