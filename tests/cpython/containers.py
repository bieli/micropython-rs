a = [1, 2, 3]
b = a
b.append(4)
print(a, len(a), a[0], a[-1], a[1:3], a[:0], a[2:])
t = (1, 'two', (3,), [])
print(t, len(t), t[1], (), (5,), t[-1])
d = {'a': 1, 'b': [1, 2], 3: 'x'}
d['c'] = 5
d['a'] = 10
print(d, len(d), d['b'], d[3], 'a' in d, 'z' in d, d.get('z'), d.get('z', 0))
print(list(d.keys()), list(d.values()), list(d.items()))
for k, v in d.items():
    print(k, '=', v)
for i in range(3):
    print(i)
print(list(range(5)), list(range(2, 10, 3)), list(range(5, 0, -2)), range(3), range(1, 9, 2))
print(len(range(0, 10, 3)), range(10)[3], 4 in range(0, 10, 2), 5 in range(0, 10, 2))
print([x * x for x in range(6)], [x for x in 'abc'], [c for c in 'hello' if c != 'l'])
print({k: k * 2 for k in range(3)}, {n: len(n) for n in ['aa', 'b']})
print(sum(x for x in range(10)), sum([1, 2, 3], 10), any([0, 0, 1]), all([1, 0]), all([]))
print(sorted([3, 1, 2]), sorted('cba'), sorted([(2, 'b'), (1, 'z'), (2, 'a')]))
print(min([4, 2, 8]), max('hello'), min(3, 1, 2), max([[1, 2], [1, 3]]))
print(list(enumerate('ab')), list(enumerate(['x', 'y'], 1)), list(zip([1, 2, 3], 'ab')))
print(list(reversed([1, 2, 3])), [1, 2] + [3], [0] * 3, (1,) + (2,), 2 * (7,))
print([1, 2] == [1, 2], [1, 2] != [1, 3], (1, 2) < (1, 3), [1] < [1, 0], {'a': 1} == {'a': 1})
print(1 in [1, 2], 3 not in (1, 2), [1] in [[1], 2], 'x' in ['x'])
x, y = 1, 2
x, y = y, x
print(x, y)
p, q = 'ab'
(m, n) = [5, 6]
print(p, q, m, n)
i = j = 7
print(i, j)
s = 'a,b,,c'
print(s.split(','), 'a b  c '.split(), ' x '.split(), 'a b c'.split(None, 1), 'k=v=w'.split('=', 1))
print('-'.join(['a', 'b', 'c']), ''.join('xyz'), ', '.join([]), '+'.join(('1',)))
l = [5, 3, 8]
l.sort()
l.insert(0, 1)
l.extend([9, 10])
print(l, l.pop(), l.pop(0), l, l.index(8), l.count(3))
l.remove(8)
l.reverse()
print(l, l.copy(), l)
l += [7]
l2 = l
l += (6,)
print(l, l2 is l, l == l2, [] is [])
l.clear()
print(l, bool(l), bool([0]), bool({}), bool(()), bool(range(0)))
d2 = {}
d2.setdefault('k', [])
d2['k'].append(1)
d2.update({'z': 0})
print(d2, d2.pop('z'), d2, d2.pop('q', 'none'), dict(d2), d2.copy() == d2)
nested = {'a': {'b': [1, {'c': 2}]}}
nested['a']['b'][1]['c'] += 40
print(nested, nested['a']['b'][1]['c'])
cnt = {}
for w in 'the cat the dog the end'.split():
    cnt[w] = cnt.get(w, 0) + 1
print(cnt)
grid = [[0] * 3 for _ in range(2)]
grid[1][2] = 5
print(grid, tuple([1, 2]), tuple('ab'), list((1, 2)))
total = 0
for v in [1, 2, 3, 4, 5]:
    if v == 2:
        continue
    if v == 5:
        break
    total += v
print(total)
for c in 'añb':
    print(c, ord(c))
for bt in b'AB':
    print(bt)
def f(items):
    out = []
    for it in items:
        out.append(it * 2)
    return out
print(f([1, 2]), f('ab'), f(range(2)))
print(repr([1, 'a', b'x', None, True]), str((1, 'a')), str({'q': "it's"}))
print([] == [], () == (), [1] == (1,), {} == {})
r = [n for n in range(10) if n % 3 == 0]
print(r, n)
print('end', 1 if r else 0, 'yes' if not [] else 'no')
