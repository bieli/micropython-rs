# machine: MicroPython's hardware classes (Pin, ADC, PWM, I2C, SoftI2C, UART,
# SPI, SoftSPI, Timer, WDT, RTC; and lscpu(), bench()), written in Python on top of the firmware's functions
# (pin_write, adc_read, pwm_u16, i2c_write, uart_read, spi_read, ...), so
# code from MicroPython tutorials runs unchanged. Built into the firmware
# (a frozen module); a machine.py on the board's file system comes first.
#
# Differences from MicroPython's ESP32 port, all raising ValueError rather
# than doing something else:
# * one I2C bus, one UART and one SPI bus at a time (the id picks defaults);
# * ADC: 11 dB attenuation and 12 bits only (0-3.3 V);
# * UART: 8 data bits, no parity, 1 stop bit; SPI: 8 bits, MSB first.
# Pin.irq() and Timer callbacks are soft interrupts (MicroPython's default,
# hard=False): they run between statements, while the script sleeps, and at
# the >>> prompt. hard=True is accepted and works the same way. Edge
# triggers only (no IRQ_LOW_LEVEL / IRQ_HIGH_LEVEL). RTC keeps the time while
# the board runs; it starts at 2000-01-01 after a reset or power-up (no
# battery-backed time). No *_into methods (they need bytearray) yet.
# Floats work where MicroPython takes them: time.sleep(0.5), machine values
# from int arithmetic (duty_ns, read_uv) stay ints, as on MicroPython.

PWRON_RESET = PWRON_RESET
HARD_RESET = HARD_RESET
WDT_RESET = WDT_RESET
DEEPSLEEP_RESET = DEEPSLEEP_RESET
SOFT_RESET = SOFT_RESET

reset = reset
reset_cause = reset_cause
unique_id = unique_id
lightsleep = sleep_ms


def freq(hz=None):
    if hz is None:
        return cpu_freq()
    if hz != cpu_freq():
        # Set when the firmware is built: make flash CPU_MHZ=80/160/240
        raise ValueError('the CPU clock is fixed at %d Hz (build with CPU_MHZ=%d)'
                         % (cpu_freq(), hz // 1000000))


# The callbacks of Pin.irq() and Timer, by event number (a pin's GPIO
# number, or 100 + a timer's slot): (callback, its argument)
_callbacks = {}


def _event(n):
    # Called by the interpreter for each event (see _irq_handler)
    entry = _callbacks.get(n)
    if entry is not None and entry[0] is not None:
        entry[0](entry[1])


def _pin(pin):
    # A Pin object or a pin number
    if isinstance(pin, Pin):
        return pin._id
    return pin


class Pin:
    IN = 1
    OUT = 3
    OPEN_DRAIN = 7
    PULL_UP = 2
    PULL_DOWN = 1

    def __init__(self, id, mode=-1, pull=-1, value=None):
        self._id = id
        self.init(mode, pull, value)

    def init(self, mode=-1, pull=-1, value=None):
        if mode == Pin.OUT:
            pin_mode(self._id, OUT)
        elif mode == Pin.OPEN_DRAIN:
            pin_mode(self._id, OUT_OD_PULLUP if pull == Pin.PULL_UP else OUT_OD)
        elif mode == Pin.IN or (mode == -1 and pull != -1):
            if pull == Pin.PULL_UP:
                pin_mode(self._id, IN_PULLUP)
            elif pull == Pin.PULL_DOWN:
                pin_mode(self._id, IN_PULLDOWN)
            else:
                pin_mode(self._id, IN)
        elif mode != -1:
            raise ValueError('invalid pin mode')
        if value is not None:
            pin_write(self._id, 1 if value else 0)

    def value(self, v=None):
        if v is None:
            return pin_read(self._id)
        pin_write(self._id, 1 if v else 0)

    def __call__(self, v=None):
        return self.value(v)

    def on(self):
        pin_write(self._id, 1)

    def off(self):
        pin_write(self._id, 0)

    def toggle(self):
        pin_toggle(self._id)

    IRQ_RISING = 1
    IRQ_FALLING = 2
    IRQ_LOW_LEVEL = 4
    IRQ_HIGH_LEVEL = 5

    def irq(self, handler=None, trigger=3, priority=1, wake=None, hard=False):
        # handler(pin) runs soon after the edge (trigger: IRQ_RISING,
        # IRQ_FALLING, or both: IRQ_RISING | IRQ_FALLING, the default)
        if handler is None:
            pin_irq(self._id, 0)
            _callbacks.pop(self._id, None)
            return
        if trigger not in (1, 2, 3):
            raise ValueError('only IRQ_RISING and IRQ_FALLING (no level triggers)')
        _callbacks[self._id] = (handler, self)
        _irq_handler(_event)
        pin_irq(self._id, trigger)

    def __repr__(self):
        return 'Pin(%d)' % self._id


class ADC:
    ATTN_0DB = 0
    ATTN_2_5DB = 1
    ATTN_6DB = 2
    ATTN_11DB = 3
    WIDTH_9BIT = 0
    WIDTH_10BIT = 1
    WIDTH_11BIT = 2
    WIDTH_12BIT = 3

    def __init__(self, pin, atten=3):
        self._id = _pin(pin)
        adc_read(self._id)          # ValueError for a pin without an ADC
        self.atten(atten)

    def read_u16(self):
        return (adc_read(self._id) * 65535 + 2047) // 4095

    def read(self):
        return adc_read(self._id)

    def read_uv(self):
        return adc_mv(self._id) * 1000      # calibrated (eFuse Vref)

    def atten(self, atten):
        if atten != ADC.ATTN_11DB:
            raise ValueError('only ADC.ATTN_11DB (0-3.3 V)')

    def width(self, width):
        if width != ADC.WIDTH_12BIT:
            raise ValueError('only ADC.WIDTH_12BIT')


class PWM:
    def __init__(self, pin, freq=5000, duty=None, duty_u16=None, duty_ns=None):
        self._id = _pin(pin)
        self._freq = freq
        self._duty = 32768
        self.init(freq, duty, duty_u16, duty_ns)

    def init(self, freq=None, duty=None, duty_u16=None, duty_ns=None):
        if freq is not None:
            self._freq = freq
        if duty is not None:
            self._duty = duty * 65535 // 1023
        if duty_u16 is not None:
            self._duty = duty_u16
        if duty_ns is not None:
            self._duty = duty_ns * self._freq * 65535 // 1000000000
        pwm_u16(self._id, self._freq, self._duty)

    def freq(self, value=None):
        if value is None:
            return self._freq
        self.init(freq=value)

    def duty(self, value=None):
        if value is None:
            return (self._duty * 1023 + 32767) // 65535
        self.init(duty=value)

    def duty_u16(self, value=None):
        if value is None:
            return self._duty
        self.init(duty_u16=value)

    def duty_ns(self, value=None):
        if value is None:
            return self._duty * 1000000000 // (65535 * self._freq)
        self.init(duty_ns=value)

    def deinit(self):
        pwm_stop(self._id)


def _mem(memaddr, addrsize):
    # A register address as the bytes sent before reading or writing it
    if addrsize == 16:
        return bytes([memaddr >> 8 & 255, memaddr & 255])
    return bytes([memaddr & 255])


class I2C:
    # One bus: the id is accepted, the pins default to the chip's (I2C_SDA,
    # I2C_SCL: 21 and 22 on the ESP32, 5 and 6 on the ESP32-S3)
    def __init__(self, id=0, scl=None, sda=None, freq=400000, timeout=None):
        if id not in (0, 1):
            raise ValueError('I2C id must be 0 or 1')
        i2c_init(I2C_SDA if sda is None else _pin(sda), I2C_SCL if scl is None else _pin(scl), freq)

    def scan(self):
        return list(i2c_scan())

    def readfrom(self, addr, nbytes, stop=True):
        return i2c_read(addr, nbytes)

    def writeto(self, addr, buf, stop=True):
        return i2c_write(addr, buf)

    def readfrom_mem(self, addr, memaddr, nbytes, addrsize=8):
        return i2c_write_read(addr, _mem(memaddr, addrsize), nbytes)

    def writeto_mem(self, addr, memaddr, buf, addrsize=8):
        i2c_write(addr, _mem(memaddr, addrsize) + buf)


class SoftI2C(I2C):
    # The hardware bus on any pins: SoftI2C(scl, sda) or SoftI2C(scl=, sda=)
    def __init__(self, scl, sda, freq=400000, timeout=None):
        I2C.__init__(self, 0, scl, sda, freq)


class UART:
    # Keyword arguments as in MicroPython: tx, rx, timeout (ms; timeout_char,
    # rxbuf, txbuf, ... are accepted and ignored)
    def __init__(self, id, baudrate=115200, bits=8, parity=None, stop=1, **kw):
        if id not in (1, 2):
            raise ValueError('UART id must be 1 or 2 (UART0 is the REPL)')
        self._uart = id
        self._buf = b''
        self.init(baudrate, bits, parity, stop, **kw)

    def init(self, baudrate=115200, bits=8, parity=None, stop=1, **kw):
        if bits != 8 or parity is not None or stop != 1:
            raise ValueError('only 8 data bits, no parity, 1 stop bit')
        tx, rx = kw.get('tx'), kw.get('rx')
        if tx is None or rx is None:
            if self._uart == 1:
                raise ValueError('UART(1) needs tx and rx pins')
            tx, rx = 17, 16         # UART(2)'s pins on MicroPython
        self._timeout = kw.get('timeout', 0)
        uart_init(_pin(tx), _pin(rx), baudrate)

    def write(self, buf):
        return uart_write(buf)

    def read(self, nbytes=512):
        data = self._buf
        self._buf = b''
        if len(data) < nbytes:
            data += uart_read(nbytes - len(data), self._timeout)
        else:
            self._buf = data[nbytes:]
            data = data[:nbytes]
        return data if data else None

    def readline(self):
        while b'\n' not in self._buf:
            more = uart_read(512, self._timeout)
            if not more:
                break
            self._buf += more
        end = self._buf.find(b'\n') + 1
        if end == 0:
            end = len(self._buf)
        line = self._buf[:end]
        self._buf = self._buf[end:]
        return line if line else None

    def any(self):
        return len(self._buf) + uart_any()


class SPI:
    MSB = 0
    LSB = 1

    # Keyword arguments as in MicroPython: polarity, phase, bits, firstbit,
    # sck, mosi, miso
    def __init__(self, id=1, baudrate=1000000, **kw):
        self._spi = id
        self.init(baudrate, **kw)

    def init(self, baudrate=1000000, **kw):
        if kw.get('bits', 8) != 8 or kw.get('firstbit', SPI.MSB) != SPI.MSB:
            raise ValueError('only 8 bits, MSB first')
        # MicroPython's default pins: SPI(1) 14, 13, 12; SPI(2) 18, 23, 19
        pins = (14, 13, 12) if self._spi == 1 else (18, 23, 19)
        sck, mosi, miso = [_pin(kw.get(k, p)) for k, p in zip(('sck', 'mosi', 'miso'), pins)]
        mode = kw.get('polarity', 0) * 2 + kw.get('phase', 0)
        spi_init(sck, mosi, miso, max(10, baudrate // 1000), mode)

    def read(self, nbytes, write=0):
        return spi_read(nbytes, write)

    def write(self, buf):
        spi_write(buf)


SoftSPI = SPI


class Timer:
    # Software timers with MicroPython's API: ids 0-3 (and -1)
    ONE_SHOT = 0
    PERIODIC = 1

    def __init__(self, id=-1, mode=1, period=-1, freq=-1, callback=None, tick_hz=1000):
        self._id = id
        self._event = 104 if id == -1 else 100 + id
        if callback is not None or period >= 0 or freq > 0:
            self.init(mode, period, freq, callback, tick_hz)

    def init(self, mode=1, period=-1, freq=-1, callback=None, tick_hz=1000):
        # period in 1/tick_hz seconds (ms by default), or freq in Hz
        if freq > 0:
            us = int(1000000 / freq)
        elif period >= 0:
            us = int(period * 1000000 / tick_hz)
        else:
            raise ValueError('Timer needs period= or freq=')
        _callbacks[self._event] = (callback, self)
        _irq_handler(_event)
        timer_init(self._id, max(us, 1), mode)

    def deinit(self):
        timer_deinit(self._id)
        _callbacks.pop(self._event, None)


class RTC:
    # datetime() is (year, month, day, weekday, hours, minutes, seconds,
    # subseconds in microseconds), as on MicroPython's ESP32 port; weekday 0
    # is Monday (it is ignored when setting)
    def __init__(self, id=0, datetime=None):
        if datetime is not None:
            self.datetime(datetime)

    def init(self, datetime):
        self.datetime(datetime)

    def datetime(self, datetimetuple=None):
        import time
        if datetimetuple is None:
            us = time.time_ns() // 1000
            t = time.gmtime(us // 1000000)
            return (t[0], t[1], t[2], t[6], t[3], t[4], t[5], us % 1000000)
        d = datetimetuple
        secs = time.mktime((d[0], d[1], d[2], d[4], d[5], d[6], 0, 0))
        rtc_set(secs, d[7] if len(d) > 7 else 0)


# lscpu(): the chip, CPU, memory and build as a dict, with the names Linux's
# lscpu uses where they fit; lscpu(True) prints them as a table instead.
# Not in MicroPython (its closest are os.uname() and machine.freq()).
def lscpu(show=False):
    # The firmware gives the dict (cpu_info); the Python heap's entries are
    # filled in here
    info = cpu_info()
    try:
        import gc
        info['Python heap size'] = gc.mem_free() + gc.mem_alloc()
        info['Python heap free'] = gc.mem_free()
    except ImportError:
        del info['Python heap size']
        del info['Python heap free']
    if not show:
        return info
    for name in info:
        value = info[name]
        if isinstance(value, int) and (name.endswith('size') or name.endswith('free')):
            if value >= 1048576 and value % 1048576 == 0:
                value = '%d MiB' % (value // 1048576)
            else:
                value = '%d KiB' % (value // 1024)
        print('%-18s %s' % (name + ':', value))


# bench(): speed tests as a dict {name with unit: value}; bench(True) prints
# a table. Native tests (cpu_bench in the firmware) show what the chip does
# without the interpreter: integer and float arithmetic (f32 and f64 both,
# whatever the build's float), memory and flash, and with a pin, GPIO.
# Python tests show what a script gets. gpio: a free pin to toggle (default:
# the board's LED, if it has one; None on the TTGO); it ends low. Names are
# literals and values plain numbers: no heap objects per result.
def bench(show=False, gpio=-1):
    import time
    tests = [('i32_add', 'native i32 add (Mop/s)'), ('i32_mul', 'native i32 mul (Mop/s)')]
    tests += [('i32_div', 'native i32 div (Mop/s)'), ('i64_add', 'native i64 add (Mop/s)')]
    tests += [('i64_mul', 'native i64 mul (Mop/s)'), ('i64_div', 'native i64 div (Mop/s)')]
    tests += [('clz', 'native count leading zeros (Mop/s)'), ('f32_add', 'native f32 add (Mop/s)')]
    tests += [('f32_mul', 'native f32 mul (Mop/s)'), ('f32_div', 'native f32 div (Mop/s)')]
    tests += [('f32_sqrt', 'native f32 sqrt (Mop/s)'), ('f32_sin', 'native f32 sin (Mop/s)')]
    tests += [('f64_add', 'native f64 add (Mop/s)'), ('f64_mul', 'native f64 mul (Mop/s)')]
    tests += [('f64_div', 'native f64 div (Mop/s)'), ('f64_sqrt', 'native f64 sqrt (Mop/s)')]
    tests += [('f64_sin', 'native f64 sin (Mop/s)')]
    results = {}
    for key, name in tests:
        results[name] = round(cpu_bench(key) / 1e6, 2)
    tests = [('ram_write', 'native RAM write (MiB/s)'), ('ram_read', 'native RAM read (MiB/s)')]
    tests += [('ram_copy', 'native RAM copy (MiB/s)'), ('flash_read', 'native flash read, cached (MiB/s)')]
    for key, name in tests:
        try:
            results[name] = round(cpu_bench(key) / 1048576, 1)
        except ValueError:
            pass                        # no frozen module text to read
    tests = None
    if gpio == -1:
        try:
            gpio = LED
        except NameError:
            gpio = None
    if gpio is not None:
        results['native GPIO toggle, esp-hal (M/s)'] = round(cpu_bench('gpio_hal', gpio) / 1e6, 2)
        try:
            results['native GPIO toggle, register (M/s)'] = round(cpu_bench('gpio_reg', gpio) / 1e6, 2)
            results['native GPIO read, register (M/s)'] = round(cpu_bench('gpio_read', gpio) / 1e6, 2)
        except ValueError:
            pass                        # GPIO32 and up
    # The interpreter: rates in thousands per second
    n = 2000
    t = time.ticks_us()
    for i in range(n):
        pass
    results['Python for loop (k/s)'] = round(n * 1000 / max(1, time.ticks_diff(time.ticks_us(), t)), 1)
    x = 0
    t = time.ticks_us()
    for i in range(n):
        x = (x * 3 + i) % 1000
    results['Python int math (x*3+i)%k (k/s)'] = round(n * 1000 / max(1, time.ticks_diff(time.ticks_us(), t)), 1)
    y = 1.0
    t = time.ticks_us()
    for i in range(n):
        y = y * 1.0001 + 0.5
    results['Python float math y*a+b (k/s)'] = round(n * 1000 / max(1, time.ticks_diff(time.ticks_us(), t)), 1)
    f = lambda a: a
    t = time.ticks_us()
    for i in range(n):
        f(i)
    results['Python function call (k/s)'] = round(n * 1000 / max(1, time.ticks_diff(time.ticks_us(), t)), 1)
    f = None
    items = []                          # 500: a long list would fill the 32 KB heap
    t = time.ticks_us()
    for i in range(500):
        items.append(i)
    results['Python list append (k/s)'] = round(500 * 1000 / max(1, time.ticks_diff(time.ticks_us(), t)), 1)
    items = None
    d = {}
    t = time.ticks_us()
    for i in range(n):
        d[i & 63] = i
    results['Python dict set (k/s)'] = round(n * 1000 / max(1, time.ticks_diff(time.ticks_us(), t)), 1)
    d = None
    t = time.ticks_us()
    for i in range(500):
        s = '%d:%d' % (i, i)
    results['Python str format (k/s)'] = round(500 * 1000 / max(1, time.ticks_diff(time.ticks_us(), t)), 1)
    if gpio is not None:
        t = time.ticks_us()
        for i in range(n):
            pin_write(gpio, i & 1)
        results['Python pin_write (k/s)'] = round(n * 1000 / max(1, time.ticks_diff(time.ticks_us(), t)), 1)
        p = Pin(gpio, Pin.OUT)
        t = time.ticks_us()
        for i in range(500):
            p.value(i & 1)
        results['Python Pin.value (k/s)'] = round(500 * 1000 / max(1, time.ticks_diff(time.ticks_us(), t)), 1)
        pin_write(gpio, 0)
    if not show:
        return results
    print('%-38s %10s' % ('test at %d MHz' % (freq() // 1000000), 'speed'))
    for name in results:
        print('%-38s %10s' % (name, results[name]))


class WDT:
    def __init__(self, id=0, timeout=5000):
        wdt_enable(timeout)

    def feed(self):
        wdt_feed()
