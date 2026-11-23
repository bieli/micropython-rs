def greet(name, greeting='Hello', punct='!'):
    return greeting + ', ' + name + punct
print(greet('Ann'), greet('Bob', 'Hi'), greet('Cy', punct='?'), greet(greeting='Yo', name='Di'))
print('a', 'b', 'c', sep='-')
print('no newline', end='')
print(' <- same line', end='\n')
print('x', 'y', sep='', end='|\n')
print(sep=':')
print(1, 2, sep=None, end=None)
f = greet
print(f('Ed'), f is greet, greet == f)
def apply(fn, value):
    return fn(value)
def double(x):
    return x * 2
print(apply(double, 21), apply(lambda v: v + 1, 1), (lambda: 'no args')())
square = lambda n: n * n
print(square(7), [square(i) for i in range(4)])
pairs = [(1, 'b'), (3, 'a'), (2, 'c')]
print(sorted(pairs, key=lambda p: p[1]), sorted(pairs, reverse=True), sorted([3, 1, 2], key=None))
words = ['banana', 'Fig', 'apple', 'cherry']
print(sorted(words, key=len), sorted(words, key=lambda w: w.lower()), sorted(words, key=len, reverse=True))
nums = [5, 2, 8, 1]
nums.sort(reverse=True)
print(nums)
nums.sort(key=lambda n: -n)
print(nums)
handlers = {'up': lambda: 'moving up', 'down': lambda: 'moving down'}
print(handlers['up'](), handlers['down']())
def counter(start=0, step=1):
    out = []
    n = start
    while len(out) < 3:
        out.append(n)
        n += step
    return out
print(counter(), counter(10), counter(step=5), counter(1, step=2))
def defaults_once(x, acc=[]):
    acc.append(x)
    return acc
print(defaults_once(1), defaults_once(2))
ops = [double, square, abs]
print([op(-3) for op in ops])
for name, func in [('d', double), ('s', square)]:
    print(name, func(4))
check = lambda a, b: a if a > b else b
print(check(3, 9), check(9, 3))
try:
    greet()
except TypeError:
    print('missing argument')
try:
    greet('a', name='b')
except TypeError:
    print('multiple values')
try:
    greet('a', colour='red')
except TypeError:
    print('unexpected keyword')
try:
    double(1, 2)
except TypeError:
    print('too many')
try:
    len([1], key=1)
except TypeError:
    print('no kwargs for len')
