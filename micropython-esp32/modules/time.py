# time: MicroPython's time module, over the firmware's native `utime` (the
# delays, ticks, the clock and the calendar: gmtime() and mktime() are Rust,
# micropython_rs::calendar).
# Built into the firmware (a frozen module).
#
# As on MicroPython's ESP32 port, time() counts seconds since 2000-01-01
# 00:00 UTC (its epoch), and localtime() is gmtime(): the board has no time
# zone. The clock starts at 2000-01-01 on boot; ntptime.settime() (Wi-Fi) or
# machine.RTC().datetime(...) sets it. A time tuple is
# (year, month, mday, hour, minute, second, weekday, yearday), weekday 0 is
# Monday, yearday 1 is January 1st.

from utime import sleep, sleep_ms, sleep_us, ticks_ms, ticks_us, ticks_diff, ticks_add
from utime import time, time_ns, gmtime, localtime, _mktime


def mktime(t):
    # Seconds since 2000-01-01 of a time tuple (weekday and yearday are
    # ignored; a month out of 1-12 moves the year, as on MicroPython)
    return _mktime(t[0], t[1], t[2], t[3], t[4], t[5])
