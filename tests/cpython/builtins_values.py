print(isinstance(5, int), isinstance(True, int), isinstance('a', str), isinstance([], (dict, list)), isinstance(5, str))
print(isinstance(None, int), isinstance(b'x', bytes), isinstance((1,), tuple), isinstance({}, dict), isinstance(range(2), range))
print(type(5) == int, type('a') is str, type([]) == list, type(None), type(1), type('x'), type(len), int, str, len)
print(type(5).__name__, type([]).__name__, int.__name__, ValueError.__name__, len.__name__)
try:
    raise KeyError('k')
except Exception as e:
    print(type(e), type(e).__name__, isinstance(e, LookupError), isinstance(e, ValueError), isinstance(e, (TypeError, KeyError)))
def f():
    pass
print(callable(f), callable(len), callable(int), callable(5), callable('x'), callable(lambda: 1), callable(ValueError))
print(f.__name__, (lambda: 0).__name__)
print(hasattr('abc', 'upper'), hasattr('abc', 'nope'), hasattr([], 'append'), hasattr({}, 'items'), hasattr(5, 'x'))
print(getattr('abc', 'upper')() if False else 'skip', getattr(ValueError, '__name__'), getattr(5, 'missing', 'default'))
print(__name__)
if __name__ == '__main__':
    print('main module')
x_global = 10
y_global = 'why'
g = globals()
print('x_global' in g, g['y_global'], 'nope' in g, len([k for k in g if k.endswith('_global')]))
print(sorted([k for k in globals() if k.endswith('_global')]))
print({k: v for (k, v) in globals().items() if k.endswith('_global')})
globals()['z_global'] = 3
print(z_global)
del globals()['z_global']
print('z_global' in globals())
print([n for n in dir() if n.endswith('_global')])
import builtins
print(builtins.len([1, 2]), builtins.max(3, 7), builtins.sum([1, 2, 3]), builtins.isinstance(1, builtins.int))
print(builtins.hasattr(builtins, 'print'), hasattr(builtins, 'nope'))
keys = sorted(['b', 'A', 'c'], key=str.lower) if False else sorted(['b', 'A', 'c'], key=lambda s: s.lower())
print(keys, list(map(str, [1, 2])) if False else [str(n) for n in [1, 2]])
print(max([3, 1, 2]), min('bca'), abs(-4), [abs(v) for v in (-1, 2, -3)])
