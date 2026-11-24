# Floats: literals, arithmetic, comparisons, conversions, formatting, math.
# Transcendental functions are printed to 12 decimals (libm and the C library
# CPython uses may differ in the last binary digit); everything else exactly.
import math

# Literals and repr: the shortest text that reads back as the same float
print(1.5, .5, 1., 1e3, 2.5e-3, 1_000.25, 0.1, 1e16, 1e15, 1e-4, 1e-5, 123456789.123)
print(0.1 + 0.2, 1 / 3, 2 / 3, 100 / 7, 1e22, 1.7976931348623157e308, 5e-324, -0.0, 3.0e-310)
print(repr(2.0), str(-1.25), [0.5, 1.0], (1e100,), {'k': 0.25})

# Arithmetic with ints and floats; `/` always gives a float
print(7 / 2, -7 / 2, 6 / 3, 1 + 2.0, 3 * 0.5, 2 ** 0.5, 2 ** -2, 10 - 0.5, True + 0.5)
print(7.5 // 2, -7.5 // 2, 7.5 // -2, 7.5 % 2, -7.5 % 2, 7.5 % -2, -7 % 2.5, 5 // 0.3, 5 % 0.3)
print(-2.5, +2.5, -(-1.5), abs(-3.25), 0.0 == -0.0, 1e308 * 10, -1e308 * 10)
x = 1.0
x += 0.5
x *= 3
x /= 2
x **= 2
x -= 1
x //= 1.5
print(x)

# Comparisons: exact, mixed with ints; NaN equals nothing
nan, inf = float('nan'), float('inf')
print(1 == 1.0, 2 < 2.5, 3 >= 3.0, 0.1 + 0.2 == 0.3, 1.5 != 1, -inf < -1e308 < 0 < inf)
print(nan == nan, nan != nan, nan < 1, nan > 1, nan is nan, 1.0 in [1, 2], 2 in [1.0, 2.0])
print(sorted([3, 1.5, -2, 2.25, 0]), max(1, 2.5, 2), min([0.5, -1, 3]), {1: 'a'}[1.0])

# Conversions
print(float(), float(3), float(True), float('1.5'), float('  -2e3 '), float('1_000.5'), float('inf'), float('-Infinity'))
print(int(3.9), int(-3.9), int(1e15), round(2.5), round(3.5), round(-2.5), round(0.5), round(1.5))
print(round(2.675, 2), round(1.005, 2), round(1234.5678, -2), round(-0.125, 2), round(2.5, 0), round(7, -1), round(15, -1))
print(type(1.5).__name__, isinstance(1.5, float), isinstance(1, float), bool(0.0), bool(-0.1), bool(nan))
print(divmod(7, 2), divmod(-7, 2), divmod(7.5, 2), divmod(-7.5, 2), pow(2, 0.5), pow(2, -1), pow(3, 4, 5), pow(-3, 3, 7))
for bad in ['abc', '1.5.5', '', '1e', '_1']:
    try:
        float(bad)
    except ValueError:
        print('ValueError:', repr(bad))
for name, f in [('1 / 0', lambda: 1 / 0), ('1.5 // 0', lambda: 1.5 // 0), ('2.0 % 0', lambda: 2.0 % 0),
                ('0.0 ** -1', lambda: 0.0 ** -1), ('int(nan)', lambda: int(nan)), ('int(inf)', lambda: int(inf)),
                ('2.0 ** 10000', lambda: 2.0 ** 10000), ('1.5 & 1', lambda: 1.5 & 1)]:
    try:
        f()
    except Exception as e:
        print(name, '->', type(e).__name__)

# Formatting
v = 1234.5678
print('%f %.2f %e %.3E %g %G %10.3f|%-10.1f|%+.1f %05.1f' % (v, v, v, v, v, 1e-7, v, v, v, -2.5))
print('%d %i %s %r %.0f %#.0f %g %g %g %.3g' % (3.99, -2.7, 1.5, 0.1, 2.5, 2.5, 100000, 1000000, 0.0001, 0.00001234))
print('{:.2f} {:,.2f} {:>10.3e} {:^12.4g} {:.1%} {:g} {:.3} {:.3} {} {:e}'.format(v, 1234567.891, v, v, 0.1234, 1e20, 12.0, 123.0, 1e100, 0))
print(f'{v:08.2f} {-v:+.1f} {v:_.1f} {inf:f} {nan:.2f} {1.0:.0e} {9.99:.2g} {0.000012345:.3g} {v:10} {-0.0:.1f}')
print(format(2.5), format(2.5, '.1f'), format(10, '.2f'), format(1 / 3, '.15g'), format(255, '.1e'), str(1e-7))
try:
    '{:d}'.format(1.5)
except ValueError:
    print('{:d} of a float: ValueError')

# math
print(math.pi, math.e, math.tau, math.inf, -math.inf, math.isnan(math.nan))
print(math.sqrt(2), math.sqrt(16), math.fabs(-2), math.floor(-2.5), math.ceil(2.1), math.trunc(-2.7), math.floor(3))
print(math.copysign(3, -0.0), math.fmod(7.5, 2), math.fmod(-7.5, 2), math.ldexp(0.75, 4), math.frexp(8.0), math.modf(-2.5))
print(math.hypot(3, 4), math.factorial(10), math.isfinite(1e308), math.isinf(-math.inf), math.degrees(math.pi), math.radians(180))
for name in ['sin', 'cos', 'tan', 'asin', 'acos', 'atan', 'exp', 'log', 'log2', 'log10', 'sinh', 'cosh', 'tanh', 'erf', 'erfc', 'gamma', 'lgamma']:
    print(name, '%.12f' % getattr(math, name)(0.5))
print('%.12f %.12f %.12f %.12f' % (math.atan2(1, -1), math.log(8, 2), math.pow(2, 0.5), math.exp(1)))
for name, args in [('sqrt', (-1,)), ('log', (0,)), ('log', (-1,)), ('exp', (1000,)), ('acos', (2,)), ('fmod', (1, 0)), ('pow', (0, -1))]:
    try:
        getattr(math, name)(*args)
    except (ValueError, OverflowError) as e:
        print('math.%s%s -> %s' % (name, args, type(e).__name__))

# A sensor-style calculation
readings = [2.5, 3.25, 2.75, 3.0]
mean = sum(readings) / len(readings)
var = sum((r - mean) ** 2 for r in readings) / len(readings)
print('mean %.3f, std %.4f, %d readings, celsius %.1f' % (mean, math.sqrt(var), len(readings), (98.6 - 32) * 5 / 9))
