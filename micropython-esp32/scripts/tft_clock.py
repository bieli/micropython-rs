# Uptime clock on the TTGO T-Display (FEATURES=tft). Runs 15 s, Ctrl-C stops it.
# There's no str() yet, so digits are drawn from a lookup function.
def digit(d):
    if d == 0: return '0'
    if d == 1: return '1'
    if d == 2: return '2'
    if d == 3: return '3'
    if d == 4: return '4'
    if d == 5: return '5'
    if d == 6: return '6'
    if d == 7: return '7'
    if d == 8: return '8'
    return '9'

def two_digits(x, y, n):
    text(x, y, digit(n // 10))
    text(x + 6, y, digit(n % 10))

cls()
color(255, 200, 0)
text(84, 40, 'uptime')
color(255, 255, 255)
text(102, 70, ':')
text(120, 70, ':')
start = ticks_ms()
last = -1
while ticks_diff(ticks_ms(), start) < 15000:
    secs = ticks_ms() // 1000
    if secs != last:
        two_digits(90, 70, secs // 3600 % 100)
        two_digits(108, 70, secs // 60 % 60)
        two_digits(126, 70, secs % 60)
        last = secs
    sleep_ms(50)
cls()
print('clock done')
