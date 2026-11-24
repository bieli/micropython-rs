def total(*nums):
    return sum(nums)
print(total(), total(1), total(1, 2, 3))
def show(first, *rest, **opts):
    return (first, rest, sorted(opts.items()))
print(show(1), show(1, 2, 3), show(1, 2, x=5, y=6), show(first=9))
def options(**kw):
    return kw
print(options(), options(a=1, b='two'))
args = [3, 4]
print(total(*args), total(1, *args), total(*args, 10), show(*args, **{'k': 1}))
conf = {'sep': '-', 'end': '!\n'}
print('a', 'b', **conf)
print(*['x', 'y', 'z'])
print(max(*[4, 9, 2]), '{a}+{b}'.format(**{'a': 1, 'b': 2}))
def make_adder(n):
    def add(x):
        return x + n
    return add
add5 = make_adder(5)
add10 = make_adder(10)
print(add5(1), add10(1), make_adder(0)(7))
def multiplier(k):
    return lambda v: v * k
triple = multiplier(3)
print(triple(4), [multiplier(i)(2) for i in range(4)])
def logged(fn):
    def wrapper(*a, **kw):
        result = fn(*a, **kw)
        print('called', fn.__name__, a, sorted(kw.items()), '->', result)
        return result
    return wrapper
@logged
def area(w, h=1):
    return w * h
print(area(3, h=4), area(2))
def repeat(times):
    def decorate(fn):
        def run(*a):
            return [fn(*a) for _ in range(times)]
        return run
    return decorate
@repeat(3)
def hello(name):
    return 'hi ' + name
print(hello('bob'))
def counter():
    state = {'n': 0}
    def bump():
        state['n'] += 1
        return state['n']
    return bump
c = counter()
c()
c()
print(c(), counter()())
def outer():
    x = 'outer x'
    def middle():
        def inner():
            return x
        return inner()
    return middle()
print(outer())
class Button:
    def __init__(self):
        self.handlers = []
    def on(self, fn):
        self.handlers.append(fn)
        return fn
    def press(self, *info):
        return [h(*info) for h in self.handlers]
b = Button()
@b.on
def beep(where):
    return 'beep at ' + where
b.on(lambda where: 'light at ' + where)
print(b.press('door'))
def apply_all(fns, value):
    return [f(value) for f in fns]
print(apply_all([str, len, lambda s: s * 2], 'ab'))
try:
    total(x=1)
except TypeError:
    print('no keyword for *args')
try:
    area()
except TypeError:
    print('missing argument through wrapper')
