import os
f = open('t_notes.txt', 'w')
print(f.write('line one\n'), f.write('line two\n'), f.write('zażółć\n'))
f.close()
print(f.closed)
f = open('t_notes.txt')
print(repr(f.read()))
f.close()
with open('t_notes.txt') as f:
    print(repr(f.readline()), repr(f.readline()), f.tell())
    print(repr(f.read(4)), repr(f.read()))
    print(repr(f.read()), repr(f.readline()))
with open('t_notes.txt') as f:
    for line in f:
        print('>', line.strip())
with open('t_notes.txt') as f:
    print(f.readlines())
with open('t_notes.txt', 'a') as f:
    f.write('appended\n')
print(open('t_notes.txt').read().splitlines() if False else open('t_notes.txt').read().split('\n'))
with open('t_data.bin', 'wb') as f:
    print(f.write(b'\x00\x01\xff'), f.write(bytes(3)))
with open('t_data.bin', 'rb') as f:
    data = f.read()
    print(data, len(data), data[2])
print(os.stat('t_data.bin')[6], os.stat('t_notes.txt')[6], os.stat('t_notes.txt')[0] & 0o170000 == 0o100000)
print(sorted([n for n in os.listdir() if n.startswith('t_')]))
os.rename('t_data.bin', 't_moved.bin')
print(sorted([n for n in os.listdir() if n.startswith('t_')]))
with open('t_moved.bin', 'rb') as f:
    print(f.read(2), f.read(10), f.read())
try:
    open('t_missing.txt')
except OSError as e:
    print('missing:', e.errno)
try:
    os.remove('t_missing.txt')
except OSError as e:
    print('remove missing:', e.errno)
f = open('t_notes.txt')
f.close()
try:
    f.read()
except ValueError:
    print('closed file')
try:
    with open('t_ctx.txt', 'w') as f:
        f.write('partial')
        raise RuntimeError('fail inside with')
except RuntimeError as e:
    print(e, f.closed)
print(open('t_ctx.txt').read())
def first_line(name):
    with open(name) as f:
        return f.readline().strip()
print(first_line('t_notes.txt'))
try:
    open('t_notes.txt', 'x+q')
except ValueError:
    print('bad mode')
for n in ['t_notes.txt', 't_moved.bin', 't_ctx.txt']:
    os.remove(n)
print([n for n in os.listdir() if n.startswith('t_')])
