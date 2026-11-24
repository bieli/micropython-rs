# machine: MicroPython's hardware classes (Pin, ADC, PWM, I2C, SoftI2C, UART,
# SPI, SoftSPI, WDT), written in Python on top of the firmware's functions
# (pin_write, adc_read, pwm_u16, i2c_write, uart_read, spi_read, ...), so
# code from MicroPython tutorials runs unchanged. Built into the firmware
# (a frozen module); a machine.py on the board's file system comes first.
#
# Differences from MicroPython's ESP32 port, all raising ValueError rather
# than doing something else:
# * one I2C bus, one UART and one SPI bus at a time (the id picks defaults);
# * ADC: 11 dB attenuation and 12 bits only (0-3.3 V);
# * UART: 8 data bits, no parity, 1 stop bit; SPI: 8 bits, MSB first.
# No Pin.irq, Timer, RTC or *_into methods (they need bytearray) yet.
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
    # One bus: the id is accepted, the pins default to the board's 21 and 22
    def __init__(self, id=0, scl=None, sda=None, freq=400000, timeout=None):
        if id not in (0, 1):
            raise ValueError('I2C id must be 0 or 1')
        i2c_init(21 if sda is None else _pin(sda), 22 if scl is None else _pin(scl), freq)

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


class WDT:
    def __init__(self, id=0, timeout=5000):
        wdt_enable(timeout)

    def feed(self):
        wdt_feed()
