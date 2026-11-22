greeting = 'Hello, Rust!'
answer = 42
copy = answer

print(greeting)
print('The answer is:', copy)

# Integer arithmetic: + - * / // % ** with Python precedence
width = 240
height = 135
print('Pixels:', width * height)
print('Center:', width // 2, height // 2)
print('Precedence:', 2 + 3 * 4, (2 + 3) * 4, -2 ** 2, 2 ** 3 ** 2)
print('Floor div / modulo:', -7 // 2, -7 % 3)

# Control flow, functions, comparisons and bitwise operators
def is_bit_set(value, bit):
    return (value >> bit) & 1 == 1

STATUS = 0b1010_0101
bit = 0
while bit < 8:
    if is_bit_set(STATUS, bit):
        state = 'on'
    else:
        state = 'off'
    print('bit', bit, state)
    bit += 1

def fact(n):
    if n <= 1:
        return 1
    return n * fact(n - 1)

print('7! =', fact(7), '| in range:', 0 < fact(5) <= 120, '| mask:', 0xFF & ~0x0F)
