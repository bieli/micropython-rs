import binascii
b = b'hello'
print(b, len(b), b[0], b[-1], b[1:3], b[:0], b[-3:])
print(repr(b), str(b), b + b' world', b * 2, 2 * b'ab', b * 0)
print(b'\x00\xff\n\t\\', b"it's", b'say "hi"', b'both \' and "')
print(b'\101\x41', len(b'\x00\x01\x02'), b'\q', len(b'\q'), b'\u263a', len(b'\u263a'))
print(b == b'hello', b != b'hello', b == 'hello', b < b'help', b'' == b'')
print(104 in b, 120 in b, b'ell' in b, b'' in b, b'xyz' not in b)
print(b.hex(), b'\x01\xab'.hex(), b.decode(), b.decode('utf-8'), 'zażółć'.encode())
print(len('zażółć'.encode()), 'zażółć'.encode().decode(), 'abc'.encode() == b'abc')
print(binascii.hexlify(b'\x00\x10\xfe'), binascii.unhexlify(b'48656c6c6f'), binascii.unhexlify('00ff'))
print(binascii.unhexlify(binascii.hexlify(b'round trip')))
print(bytes(), bytes(3), bytes(b'x'), bytes('ąb', 'utf-8'))
print(b.startswith(b'he'), b.endswith(b'lo'), b.find(b'l'), b.find(b'z'))
x = b''
i = 0
while i < 5:
    x += bytes(1) + b'\x41'
    i += 1
print(x, len(x))
if b'':
    print('empty is true')
if b:
    print('non-empty is true')
print(str(b'a' + b'\x00'), repr(b'\x7f\x80'))
