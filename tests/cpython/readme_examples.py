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
def logged(fn):
    def wrapper(*args, **kwargs):
        result = fn(*args, **kwargs)
        print(fn.__name__, args, '->', result)
        return result
    return wrapper
@logged
def area(w, h=1):
    return w * h
area(3, h=4)
def adc_mv(pin):
    return 2060
class Sensor:
    count = 0
    def __init__(self, pin, name='sensor'):
        self.pin = pin
        self.name = name
        Sensor.count += 1
    def read(self):
        return adc_mv(self.pin)
    def __str__(self):
        return '%s on pin %d' % (self.name, self.pin)
    @classmethod
    def made(cls):
        return cls.count
class Battery(Sensor):
    def __init__(self):
        super().__init__(35, 'battery')
    def read(self):
        return super().read() * 2
class SensorError(OSError):
    pass
bat = Battery()
print(bat, bat.read(), Sensor.made(), isinstance(bat, Sensor))
