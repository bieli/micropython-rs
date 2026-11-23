readings = [(i * 37) % 100 for i in range(8)]
print(sum(readings) // len(readings), max(readings), sorted(readings)[:3])
pins = {'led': 2, 'button': 0}
pins['buzzer'] = 25
for name, pin in pins.items():
    print(name, pin)
counts = {}
for word in 'the cat the dog'.split():
    counts[word] = counts.get(word, 0) + 1
print(counts)
x, y = 10, 20
x, y = y, x
print(x, y)
def adc_read(pin):
    return 1234
def led_off():
    print('led off')
def read_sensor(pin):
    if pin not in (32, 33, 34, 35, 36, 39):
        raise ValueError('not an ADC1 pin: ' + str(pin))
    return adc_read(pin)
try:
    mv = read_sensor(5)
except ValueError as e:
    print('config error:', e)
except OSError:
    print('sensor failed')
finally:
    led_off()
print(read_sensor(36))
