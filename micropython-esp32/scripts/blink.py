# Blink an LED on any output pin (GPIO2 on most DevKit boards)
pin_mode(2, OUT)
n = 0
while n < 10:
    pin_toggle(2)
    sleep_ms(200)
    n += 1
print('blinked', n // 2, 'times')
