# time: MicroPython's time module. The firmware's native `utime` gives the
# delays, ticks and the clock; this adds the calendar functions in Python.
# Built into the firmware (a frozen module).
#
# As on MicroPython's ESP32 port, time() counts seconds since 2000-01-01
# 00:00 UTC (its epoch), and localtime() is gmtime(): the board has no time
# zone. The clock starts at 2000-01-01 on boot; ntptime.settime() (Wi-Fi) or
# machine.RTC().datetime(...) sets it. A time tuple is
# (year, month, mday, hour, minute, second, weekday, yearday), weekday 0 is
# Monday, yearday 1 is January 1st.

from utime import sleep, sleep_ms, sleep_us, ticks_ms, ticks_us, ticks_diff, ticks_add
from utime import time, time_ns

_MONTH_DAYS = (31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31)

# Leap years are tested inline (no helper function): with Wi-Fi the stack has
# room for only about 5 nested Python calls, and gmtime() is often 2 deep.


def gmtime(secs=None):
    if secs is None:
        secs = time()
    days, secs = divmod(int(secs), 86400)
    weekday = (days + 5) % 7            # 2000-01-01 was a Saturday
    year = 2000
    while days < 0:
        year -= 1
        days += 366 if year % 4 == 0 and (year % 100 != 0 or year % 400 == 0) else 365
    while True:
        length = 366 if year % 4 == 0 and (year % 100 != 0 or year % 400 == 0) else 365
        if days < length:
            break
        days -= length
        year += 1
    yearday = days + 1
    month = 0
    while True:
        length = _MONTH_DAYS[month]
        if month == 1 and year % 4 == 0 and (year % 100 != 0 or year % 400 == 0):
            length = 29
        if days < length:
            break
        days -= length
        month += 1
    return (year, month + 1, days + 1, secs // 3600, secs // 60 % 60, secs % 60, weekday, yearday)


localtime = gmtime


def mktime(t):
    # Seconds since 2000-01-01 of a time tuple (weekday and yearday are
    # ignored; a month out of 1-12 moves the year, as on MicroPython)
    year, month = t[0] + (t[1] - 1) // 12, (t[1] - 1) % 12
    days = 0
    y = 2000
    while y < year:
        days += 366 if y % 4 == 0 and (y % 100 != 0 or y % 400 == 0) else 365
        y += 1
    while y > year:
        y -= 1
        days -= 366 if y % 4 == 0 and (y % 100 != 0 or y % 400 == 0) else 365
    m = 0
    while m < month:
        days += _MONTH_DAYS[m]
        m += 1
    if month > 1 and year % 4 == 0 and (year % 100 != 0 or year % 400 == 0):
        days += 1
    days += t[2] - 1
    return ((days * 24 + t[3]) * 60 + t[4]) * 60 + t[5]
