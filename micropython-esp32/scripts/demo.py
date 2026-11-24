# Default demo for any ESP32 board (make flash)
print('=== micropython-rs ===')
chip = 'ESP32'
a = 21
b = 21
print('Hello from', chip)
print('a =', a, 'b =', b)

result = (a + b) * 3 - 2 ** 4 // 2
print('(a+b)*3-2**4//2 =', result)

def square(n):
    return n * n

i = 1
while i <= 3:
    if i & 1:
        kind = 'odd'
    else:
        kind = 'even'
    print('square', i, '=', square(i), kind)
    i += 1

start = ticks_ms()
sleep_ms(200)
print('slept', ticks_diff(ticks_ms(), start), 'ms')
print('ADC34 raw:', adc_read(34))
print('I2C devices:', list(i2c_scan()))

print('Blinking onboard LED 3 times...')
count = 0
while count < 6:
    pin_toggle(LED)
    sleep_ms(250)
    count += 1
pwm(LED, 1000, 10)
print('LED dimmed to 10% with PWM')
