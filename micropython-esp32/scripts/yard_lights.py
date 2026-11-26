# Yard lights: switches an LED strip (one GPIO pin, through a MOSFET or a
# relay module) on at sunset and off at sunrise, every day, for months.
# Sunrise and sunset are computed on the board from the location (NOAA's
# solar equations: within a minute of astronomical tables); the clock comes from NTP over Wi-Fi,
# and the local time follows the EU summer time rules.
#
# Firmware: make flash FEATURES=tft,wifi (the screen and buttons are
# optional: on a plain ESP32 build with FEATURES=wifi it runs without them).
# Edit the configuration below, then make it the board's boot program:
#   python3 tools/mpy.py save scripts/yard_lights.py
#
# The TTGO's buttons (the screen goes dark after SCREEN_SECONDS; the first
# press only wakes it):
#   LEFT  (GPIO0)   short: wake / redraw     hold: manual on/off until the next switch
#   RIGHT (GPIO35)  short: status / details  hold: set the clock from NTP now
#
# Safe for months: a watchdog resets the board if the script hangs, the clock
# is set again from NTP every SYNC_HOURS (the board's clock drifts a few
# seconds a day), Wi-Fi reconnects when the router comes back, and without a
# clock yet the lights stay as LIGHTS_WITHOUT_CLOCK says.
#
# Kept shallow: with Wi-Fi the stack has room for about 5 nested Python calls,
# so nothing here goes more than 3 calls below the main loop.

import time
import machine
import network
import ntptime
from math import sin, cos, tan, asin, acos, radians, degrees

# --- configuration: edit before copying to the board --------------------------

WIFI_SSID = 'my-iot-network'
WIFI_PASSWORD = 'change-me'

LATITUDE = 52.4064          # degrees, north positive (Poznan)
LONGITUDE = 16.9252         # degrees, east positive
UTC_OFFSET = 1              # hours, standard (winter) time: Poland is UTC+1 (CET)
EU_SUMMER_TIME = True       # +1 hour from the last Sunday of March to the last
                            # Sunday of October, both at 01:00 UTC (CEST)

LED_PIN = 26                # a free pin on the TTGO: 25, 26, 27, 32, 33, 2, 15, 13, 12
LED_ACTIVE_HIGH = True      # False for relay modules that switch on with a low input

SUN_ANGLE = -0.833          # the sun's height at the switch: -0.833 is sunrise/sunset;
                            # -6 is civil dusk/dawn (on ~35 min later, off ~35 min earlier)
ON_DELAY_MIN = 0            # minutes after sunset to switch on (negative: before)
OFF_ADVANCE_MIN = 0         # minutes before sunrise to switch off (negative: after)
NIGHT_BREAK = None          # ((0, 30), (5, 0)): off from 00:30 to 05:00 local time; None: all night
LIGHTS_WITHOUT_CLOCK = False  # the lights before the clock is first set (no Wi-Fi at boot)

NTP_HOST = 'pl.pool.ntp.org'
SYNC_HOURS = 6              # set the clock from NTP this often
SCREEN_SECONDS = 60         # the screen goes dark after this long without a button press
WATCHDOG_MS = 60000         # the board resets if the script stops for this long

# --- the board ----------------------------------------------------------------

try:
    import display
    import buttons
except ImportError:
    display = None          # a board without the screen: no UI, status to the serial port

led = machine.Pin(LED_PIN, machine.Pin.OUT, value=0 if LED_ACTIVE_HIGH else 1)
wlan = network.WLAN(network.STA_IF)
ntptime.host = NTP_HOST
ntptime.timeout = 2

# --- time ---------------------------------------------------------------------

VALID_CLOCK = 820000000     # seconds since 2000: 2025-12; an earlier clock isn't set


def utc_offset(t):
    # Seconds from UTC to local time at `t` (seconds since 2000, UTC); sets
    # offset_from/offset_until, the times between which it stays the same
    global offset_from, offset_until
    year = time.gmtime(t)[0]
    offset_from = time.mktime((year, 1, 1, 0, 0, 0, 0, 0))
    offset_until = time.mktime((year + 1, 1, 1, 0, 0, 0, 0, 0))
    if not EU_SUMMER_TIME:
        return UTC_OFFSET * 3600
    # The last Sunday of March and of October, 01:00 UTC (weekday 6 is Sunday)
    march = time.mktime((year, 3, 31, 1, 0, 0, 0, 0))
    march -= (time.gmtime(march)[6] + 1) % 7 * 86400
    october = time.mktime((year, 10, 31, 1, 0, 0, 0, 0))
    october -= (time.gmtime(october)[6] + 1) % 7 * 86400
    if t < march:
        offset_until = march
    elif t < october:
        offset_from, offset_until = march, october
        return UTC_OFFSET * 3600 + 3600
    else:
        offset_from = october
    return UTC_OFFSET * 3600


def hhmm(t):
    # Local time of `t` (UTC seconds), 'HH:MM' (with today's UTC offset)
    minute = (t + offset) // 60 % 1440
    return '%02d:%02d' % (minute // 60, minute % 60)


# --- the sun ------------------------------------------------------------------


def sun_times(day):
    # (on, off): when the lights switch on and off on solar day `day` (days
    # since 2000-01-01 at this longitude: it starts at local solar midnight),
    # in UTC seconds. on is at sunset, off at sunrise (with the delays): the
    # lights are on before `off` and from `on`. Polar night: on all day; polar
    # day: off all day.
    lat = radians(LATITUDE)
    cos_zenith = cos(radians(90 - SUN_ANGLE))
    times = []
    for rising in (True, False):
        minutes = 720 - 4 * LONGITUDE + (-360 if rising else 360)   # first guess
        for step in range(3):
            # The sun at that moment: T is in Julian centuries from 2000-01-01
            # 12:00 UTC (small numbers: accurate with 32-bit floats too)
            T = (day - 0.5 + minutes / 1440) / 36525
            L0 = radians((280.46646 + T * (36000.76983 + T * 0.0003032)) % 360)
            M = radians(357.52911 + T * (35999.05029 - 0.0001537 * T))
            e = 0.016708634 - T * (0.000042037 + 0.0000001267 * T)
            C = (sin(M) * (1.914602 - T * (0.004817 + 0.000014 * T))
                 + sin(2 * M) * (0.019993 - 0.000101 * T) + sin(3 * M) * 0.000289)
            omega = radians(125.04 - 1934.136 * T)
            longitude = L0 + radians(C - 0.00569 - 0.00478 * sin(omega))
            eps = radians(23 + (26 + (21.448 - T * (46.815 + T * (0.00059 - T * 0.001813))) / 60) / 60
                          + 0.00256 * cos(omega))
            decl = asin(sin(eps) * sin(longitude))
            y = tan(eps / 2) ** 2
            eqtime = 4 * degrees(y * sin(2 * L0) - 2 * e * sin(M) + 4 * e * y * sin(M) * cos(2 * L0)
                                 - 0.5 * y * y * sin(4 * L0) - 1.25 * e * e * sin(2 * M))
            x = cos_zenith / (cos(lat) * cos(decl)) - tan(lat) * tan(decl)
            noon = 720 - 4 * LONGITUDE - eqtime
            if x > 1:                   # the sun stays below: night all day
                minutes = noon
                break
            if x < -1:                  # the sun stays up: day all day
                minutes = noon + (-720 if rising else 720)
                break
            half_day = 4 * degrees(acos(x))
            minutes = noon - half_day if rising else noon + half_day
        times.append(day * 86400 + int(minutes * 60))
    if times[0] == times[1]:            # polar night
        return (times[0] - 86400, times[0] - 86400)
    return (times[1] + ON_DELAY_MIN * 60, times[0] - OFF_ADVANCE_MIN * 60)


# --- state --------------------------------------------------------------------

lights = None               # what the pin drives now
auto = None                 # what the schedule says
manual = None               # True/False: a manual switch, until the schedule changes
reason = 'starting'
today = -1                  # the solar day of `sun`
sun = (0, 0)                # (on, off) of today, and of tomorrow
sun_next = (0, 0)
phase = None                # (day, 0/1/2: before sunrise/daylight/after sunset) of next_text
next_text = ''              # 'next: off 07:24'
offset_from = 0             # `offset` holds from offset_from to offset_until
offset_until = 0
offset = 0                  # utc_offset() now
synced = None               # time.time() of the last NTP sync
next_sync = 0               # time.ticks_ms() of the next try
sync_wait = 60000           # ms between tries while they fail (doubles up to 30 min)
status = 'clock not set'
screen_on = True
last_press = time.ticks_ms()
page = 0                    # 0: status, 1: details
shown = None                # what the screen shows (redrawn when it changes)


def set_lights(on, why):
    global lights, reason
    reason = why
    if on != lights:
        led.value(1 if on == LED_ACTIVE_HIGH else 0)
        lights = on
        if display is None:
            print('lights', 'on' if on else 'off', '-', why)


def sync_clock():
    # Connects (if needed) and sets the clock from NTP; False when it fails
    global synced, status
    if not wlan.isconnected():
        wlan.active(True)
        wlan.connect(WIFI_SSID, WIFI_PASSWORD)
        start = time.ticks_ms()
        while not wlan.isconnected() and time.ticks_diff(time.ticks_ms(), start) < 20000:
            time.sleep_ms(200)          # feeds the watchdog, polls Wi-Fi
        if not wlan.isconnected():
            code = wlan.status()
            if code == network.STAT_WRONG_PASSWORD:
                status = 'Wi-Fi: wrong password'
            elif code == network.STAT_NO_AP_FOUND:
                status = 'Wi-Fi: network not found'
            else:
                status = 'no Wi-Fi (status %d)' % code
            return False
    for attempt in range(3):
        try:
            ntptime.settime()
            synced = time.time()
            status = 'clock set by NTP'
            return True
        except OSError:
            time.sleep(2)               # an NTP server may ignore quick repeats
    status = 'NTP: no answer'
    return False


def schedule():
    # The lights the schedule wants now, and why
    global today, sun, sun_next, offset, manual, auto, next_text, phase
    now = time.time()
    if now < VALID_CLOCK:
        return (LIGHTS_WITHOUT_CLOCK, 'no clock yet')
    if not offset_from <= now < offset_until:   # a new year, summer time, or a new clock
        offset = utc_offset(now)
    day = int(now + LONGITUDE * 240) // 86400   # the solar day
    if day != today:
        sun = sun_times(day)
        sun_next = sun_times(day + 1)
        today = day
    on, off = sun
    want = now < off or now >= on
    why = 'after sunset' if want else 'daylight'
    now_phase = (day, 0 if now < off else 1 if now < on else 2)
    if now_phase != phase:
        if now < off:
            next_text = 'next: off %s' % hhmm(off)
        elif now < on:
            next_text = 'next: on %s' % hhmm(on)
        else:
            next_text = 'next: off %s' % hhmm(sun_next[1])
        phase = now_phase
    if want and NIGHT_BREAK is not None:
        minute = (now + offset) // 60 % 1440     # local time of day
        start = NIGHT_BREAK[0][0] * 60 + NIGHT_BREAK[0][1]
        end = NIGHT_BREAK[1][0] * 60 + NIGHT_BREAK[1][1]
        if start <= end:
            pause = start <= minute < end
        else:
            pause = minute >= start or minute < end    # a break across midnight
        sunrise_off = off if now < off else sun_next[1]
        if pause:
            want = False
            why = 'night break'
            # On again at the break's end, unless the sun is up by then
            if now + (end - minute) % 1440 * 60 < sunrise_off:
                next_text = 'next: on %02d:%02d' % NIGHT_BREAK[1]
            else:
                next_text = 'next: on %s' % hhmm(on if now < on else sun_next[0])
        elif now + (start - minute) % 1440 * 60 < sunrise_off:
            next_text = 'next: off %02d:%02d' % NIGHT_BREAK[0]   # the break comes first
        phase = None                    # worked out again when the break is over
    if want != auto:
        manual = None                   # the schedule changed: back to automatic
        auto = want
    if manual is not None:
        return (manual, 'manual')
    return (want, why)


# --- the screen (calculator style) --------------------------------------------
# Called from the main loop only: each draws, at most 2 calls deeper.

TOP = 24
TILE_BG = display.rgb(40, 40, 70) if display is not None else 0


def draw_title():
    display.fill_rect(0, 0, 240, TOP - 2, display.BLACK)
    display.font(2)
    display.text(4, 4, 'Yard lights', display.CYAN)
    clock = hhmm(time.time()) if time.time() >= VALID_CLOCK else '--:--'
    w = display.text_width(clock)
    display.text(236 - w, 4, clock, display.WHITE)


def draw_status():
    # The big state tile, today's sun times, the clock and Wi-Fi
    display.fill_rect(0, TOP, 240, 135 - TOP, display.BLACK)
    bg = display.ORANGE if lights else TILE_BG
    display.fill_rect(1, TOP + 1, 238, 50, bg)
    display.font(3)
    display.text(10, TOP + 10, 'ON' if lights else 'OFF', display.WHITE if lights else display.GRAY, bg)
    display.font(1)
    display.text(110, TOP + 12, reason, display.WHITE, bg)
    if time.time() >= VALID_CLOCK:
        display.text(110, TOP + 30, next_text, display.WHITE, bg)
        on, off = sun
        display.fill_rect(1, TOP + 53, 238, 26, TILE_BG)
        display.font(2)
        display.text(8, TOP + 58, 'sunrise %s  sunset %s' % (hhmm(off + OFF_ADVANCE_MIN * 60), hhmm(on - ON_DELAY_MIN * 60)), display.WHITE, TILE_BG)
    display.font(1)
    display.text(8, TOP + 88, status, display.YELLOW if synced is None else display.GRAY)
    if wlan.isconnected():
        display.text(156, TOP + 88, 'Wi-Fi %d dBm' % wlan.status('rssi'), display.GRAY)


def draw_details():
    display.fill_rect(0, TOP, 240, 135 - TOP, display.BLACK)
    display.fill_rect(1, TOP + 1, 238, 135 - TOP - 2, TILE_BG)
    display.font(1)
    # One statement per row: a single expression of all six is too big to parse
    hours = offset // 3600
    zone = 'UTC%s%d' % ('+' if hours >= 0 else '-', abs(hours))
    if offset != UTC_OFFSET * 3600:
        zone += ' (summer time)'
    up = time.ticks_ms() // 60000
    display.text(8, TOP + 6, 'location   %.4f N %.4f E' % (LATITUDE, LONGITUDE), display.WHITE, TILE_BG)
    display.text(8, TOP + 22, 'time zone  ' + zone, display.WHITE, TILE_BG)
    display.text(8, TOP + 38, 'LED        GPIO%d %s' % (LED_PIN, 'on' if lights else 'off'), display.WHITE, TILE_BG)
    display.text(8, TOP + 54, 'last sync  ' + (hhmm(synced) if synced else 'never'), display.WHITE, TILE_BG)
    ip = wlan.ifconfig()[0] if wlan.isconnected() else 'not connected'
    display.text(8, TOP + 70, 'Wi-Fi      ' + ip, display.WHITE, TILE_BG)
    display.text(8, TOP + 86, 'running    %d h %d min' % (up // 60, up % 60), display.WHITE, TILE_BG)


# --- main loop ----------------------------------------------------------------

print('yard lights: LED on GPIO%d, %.4f N %.4f E' % (LED_PIN, LATITUDE, LONGITUDE))
if display is not None:
    display.fill(display.BLACK)         # clears the banner off the screen
set_lights(LIGHTS_WITHOUT_CLOCK, 'no clock yet')
wdt = machine.WDT(timeout=WATCHDOG_MS)
if time.time() >= VALID_CLOCK:
    synced = time.time()                # set by an earlier script since the last reset
    status = 'clock set (kept)'
    next_sync = time.ticks_add(time.ticks_ms(), 60000)

while True:
    wdt.feed()
    try:
        if time.ticks_diff(time.ticks_ms(), next_sync) >= 0:
            if display is not None and screen_on:
                status = 'setting the clock...'
                draw_title()
                draw_status()
            if sync_clock():
                next_sync = time.ticks_add(time.ticks_ms(), SYNC_HOURS * 3600000)
                sync_wait = 60000
            else:
                next_sync = time.ticks_add(time.ticks_ms(), sync_wait)
                sync_wait = min(sync_wait * 2, 1800000)
            shown = None
        want, why = schedule()
        set_lights(want, why)
    except OSError as e:
        # Wi-Fi trouble mid-way: keep going (anything else ends the script,
        # and the watchdog then restarts the board)
        status = 'error: %s' % e
        next_sync = time.ticks_add(time.ticks_ms(), sync_wait)

    if display is None:
        time.sleep(1)
        continue
    event = buttons.wait(1000)          # a 1 s tick; feeds the watchdog meanwhile
    if event != buttons.NONE:
        last_press = time.ticks_ms()
        if not screen_on:
            display.backlight(1)        # the first press only wakes the screen
            screen_on = True
            shown = None
            continue
        if event == buttons.LEFT_LONG:
            manual = not lights
            want, why = schedule()
            set_lights(want, why)
        elif event == buttons.RIGHT:
            page = 1 - page
        elif event == buttons.RIGHT_LONG:
            next_sync = time.ticks_ms()
        shown = None
    elif screen_on and time.ticks_diff(time.ticks_ms(), last_press) > SCREEN_SECONDS * 1000:
        display.backlight(0)
        screen_on = False
    # Redraw when something on it changes (the clock: every minute)
    key = (lights, reason, status, page, time.time() // 60)
    if screen_on and key != shown:
        draw_title()
        if page == 0:
            draw_status()
        else:
            draw_details()
        shown = key
