# MicroPython's machine module, as in its tutorials. Checks itself with two
# jumper wires: GPIO25 to itself is enough for the UART (TX and RX on one pin),
# and GPIO27 for SPI (MOSI and MISO on one pin); without them those two lines
# print False.
import time
from machine import Pin, ADC, PWM, I2C, UART, SPI, WDT
import machine

led = Pin(2, Pin.OUT)
for i in range(3):
    led.value(not led.value())
    time.sleep_ms(100)
led.off()
print('led', led, led.value())

button = Pin(0, Pin.IN, Pin.PULL_UP)
print('BOOT button pressed:', button.value() == 0)

adc = ADC(Pin(34))
adc.atten(ADC.ATTN_11DB)
print('ADC34: raw %d, u16 %d, %d uV' % (adc.read(), adc.read_u16(), adc.read_uv()))

pwm = PWM(Pin(2), freq=1000, duty_u16=16384)    # 25 %
print('PWM', pwm.freq(), 'Hz, duty', pwm.duty(), '/ 1023,', pwm.duty_ns(), 'ns')
servo = PWM(Pin(13), freq=50)
servo.duty_ns(1500000)                          # 1.5 ms: the middle position
print('servo duty_u16', servo.duty_u16())
pwm.deinit()
servo.deinit()

i2c = I2C(0, scl=Pin(22), sda=Pin(21), freq=400000)
print('I2C devices:', [hex(a) for a in i2c.scan()])

uart = UART(1, baudrate=9600, tx=25, rx=25, timeout=50)
uart.write(b'$GPGGA,123\r\nnext')
print('UART line:', uart.readline(), 'then', uart.read(), uart.read() is None)

spi = SPI(1, baudrate=1000000, polarity=0, phase=0, sck=Pin(26), mosi=Pin(27), miso=Pin(27))
print('SPI loopback:', spi.read(4, 0x5a) == b'\x5a\x5a\x5a\x5a')

wdt = WDT(timeout=3000)
wdt.feed()
wdt_disable()                                   # (not in MicroPython: stop it again)

print('freq', machine.freq(), 'id', machine.unique_id().hex(),
      'reset cause', machine.reset_cause())
