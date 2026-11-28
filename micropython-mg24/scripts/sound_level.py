# A sound level meter on the XIAO MG24 Sense's microphone: a bar of its
# level (rms, in dB above the quiet room it starts in) every 100 ms, and
# the LED on while it's loud. Clap, speak or whistle near the board.
#
#   python3 ../micropython-esp32/tools/mpy.py run scripts/sound_level.py
import math
import time

import mic
from machine import Pin

led = Pin(LED, Pin.OUT)
led.on()  # lit when low: on() is off on this board
mic.init()  # PC9, powered from PC8, 16 kHz
mic.level(300)  # the bias still drifts a little after power-on
quiet = 0.0
for i in range(5):
    quiet += mic.level(100)[0] / 5
print('quiet room: %.1f mV rms; bias %.0f mV' % (quiet, mic.level(20)[2]))
for i in range(50):
    rms, peak, dc = mic.level(100)
    db = 20 * math.log10(max(rms, 0.01) / quiet)
    if db > 10:
        led.off()
    else:
        led.on()
    print('%5.1f dB %6.1f mV |%s' % (db, rms, '#' * max(0, min(60, int(db * 2)))))
mic.off()
led.on()
