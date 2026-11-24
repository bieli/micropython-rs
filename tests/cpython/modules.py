# Modules are written to files first, so this runs the same on CPython and on
# micropython-rs (whose file system starts empty)
import os


def forget_file_cache():
    # CPython caches directory listings; micropython-rs has no importlib
    try:
        import importlib
        importlib.invalidate_caches()
    except ImportError:
        pass


SENSORS = '''
LIMIT = 100
count = 0
readings = []

def record(value):
    global count
    count += 1
    readings.append(value)
    return check(value)

def check(value):
    return 'high' if value > LIMIT else 'ok'

class Sensor:
    def __init__(self, pin):
        self.pin = pin
    def label(self):
        return 'pin %d (limit %d)' % (self.pin, LIMIT)

def helper_count():
    return count
'''
with open('t_sensors.py', 'w') as f:
    f.write(SENSORS)
with open('t_units.py', 'w') as f:
    f.write("import t_sensors\nFACTOR = 1000\ndef mv(v):\n    return v * FACTOR\n"
            "def limit_mv():\n    return mv(t_sensors.LIMIT)\nprint('t_units loaded, __name__ =', __name__)\n")
forget_file_cache()

import t_sensors
print(t_sensors.LIMIT, t_sensors.record(50), t_sensors.record(150), t_sensors.count, t_sensors.readings)
print(t_sensors.helper_count(), t_sensors.Sensor(36).label(), t_sensors.__name__)
LIMIT = 'main LIMIT is separate'
print(LIMIT, t_sensors.check(120))
from t_sensors import record, Sensor
print(record(1), Sensor(2).label(), t_sensors.count)
import t_units
import t_units as u
print(u.mv(3), u.limit_mv(), u is t_units, t_units.t_sensors is t_sensors)
t_sensors.LIMIT = 10
print(t_sensors.check(50), u.limit_mv())
print(type(t_sensors).__name__, hasattr(t_sensors, 'record'), hasattr(t_sensors, 'nope'))
try:
    import t_no_such_module
except ImportError:
    print('missing module: ImportError')
try:
    from t_sensors import nope
except ImportError:
    print('missing name: ImportError')
with open('t_broken.py', 'w') as f:
    f.write('x = 1\ny = 1 // 0\n')
forget_file_cache()
try:
    import t_broken
except ZeroDivisionError:
    print('error while importing')
for name in ['t_sensors.py', 't_units.py', 't_broken.py']:
    os.remove(name)
