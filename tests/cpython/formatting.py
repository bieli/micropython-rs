mv = 1234
name = 'ADC'
print('%d mV' % mv, '%5d|' % 42, '%-5d|' % 42, '%05d' % -42, '%+d %+d' % (5, -5), '% d' % 7)
print('%x %X %o %#x %#o %08X' % (255, 255, 8, 255, 8, 48879))
print('%s=%r' % (name, name), '%s' % [1, 'a'], '%s' % (None,), '%10s|' % 'hi', '%-10s|' % 'hi', '%.2s' % 'hello')
print('%c%c' % (72, 'i'), '100%%', '%(a)s and %(b)d' % {'a': 'x', 'b': 3}, '%.3d' % 5, '%5.1f|' % 3)
print('{} {}'.format('a', 'b'), '{1} {0}'.format('a', 'b'), '{x}-{y}'.format(x=1, y=2), '{{literal}}'.format())
print('{:>6}|{:<6}|{:^6}|'.format('ab', 'cd', 'ef'), '{:*^9}'.format('mid'), '{:06.2f}'.format(7) if False else '{:.2f}'.format(7))
print('{:5d}|{:<5d}|{:05d}|{:+d}'.format(42, 42, -42, 7), '{:x} {:X} {:o} {:b} {:#x} {:#b}'.format(255, 255, 8, 5, 255, 5))
print('{:,}'.format(1234567), '{:_}'.format(1000000), '{:,d}'.format(-9876543), '{:08,}'.format(12345) if False else '')
print('{!r} {!s} {:>8}'.format('q', 'q', 'right'), '{:c}'.format(65), '{:%}'.format(1) if False else '{:.0%}'.format(1))
print(format(42, '04d'), format('x', '>3'), format(255, 'x'), format(7), format([1, 2]))
v = 3
items = ['a', 'b']
print(f'v={v}', f'{v * 2} {v ** 2}', f'{name!r} {name:>5}|', f'{mv:,} mV', f'{{braces}} {v}')
print(f'{items[0]}{items[-1]}', f"{len(items)} items", f'{"nested"}', f'{v:03d} {v:x} {255:#06x}', f'')
print(f'{name.lower()}_{v}', f'{[x * 2 for x in range(3)]}', f'{ {"k": 1}["k"] }', f'{v if v > 2 else 0}')
width = 5
print(f'{"pad":>{5}}' if False else f'{"pad":>5}', f'{v!s:<4}|', f'{True} {None} {b"x"}')
t = 'T'
print(f'{t}' + f'{t}', f'a{t}b{t}c', 'x' + f'{1 + 1}')
print('{0}{1}{0}'.format('ab', 'cd'), '{}'.format(True), '{:d}'.format(True), '%s %d' % (True, True))
try:
    '%d' % 'x'
except TypeError:
    print('d needs number')
try:
    '%s %s' % ('one',)
except TypeError:
    print('not enough')
try:
    '%s' % (1, 2)
except TypeError:
    print('not all converted')
try:
    '{:d}'.format('s')
except ValueError:
    print('bad spec for str')
try:
    '{}'.format()
except IndexError:
    print('missing positional')
