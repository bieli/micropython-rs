# Default demo for the LilyGO TTGO T-Display (make flash FEATURES=tft)
print('=== micropython-rs ===')
board = 'TTGO T-Display'
a = 21
b = 21
print('Board:', board)
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
print('I2C devices:', [hex(a) for a in i2c_scan()])

# Buttons are active low: 1 = not pressed
pin_mode(BUTTON_L, IN_PULLUP)
print('buttons (1 = up):', pin_read(BUTTON_L), pin_read(BUTTON_R))

# The battery divider is only connected while ADC_EN is high
pin_write(ADC_EN, 1)
sleep_ms(10)
print('battery ADC raw:', adc_read(BAT_ADC))

# Status bar drawn below the console text
color(255, 200, 0)
rect(0, 124, WIDTH, 11)
color(0, 0, 0)
text(4, 125, 'micropython-rs Phase 3 on ESP32')
color(144, 238, 144)
print('TFT console active!')
