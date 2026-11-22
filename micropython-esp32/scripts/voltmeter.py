# Voltmeter for the LilyGO TTGO T-Display (FEATURES=tft): an analog input
# in millivolts with min/max/average and a sweeping graph, plus the battery
# voltage and charge level.
#
#   LEFT  button: switch input GPIO36 <-> GPIO39   (hold: reset min/max/avg)
#   RIGHT button: HOLD the display                 (hold: change graph range)
#
# Input range 0.15 .. ~3.1 V (ESP32 ADC at 11 dB). NEVER connect more than
# 3.3 V to an ADC pin. Readings use the chip's eFuse calibration (adc_mv);
# to match a multimeter, adjust CAL_NUM / CAL_DEN (gain) and CAL_OFFSET (mV).
import display
import buttons
import time
from display import WIDTH, HEIGHT, rgb

CAL_NUM = 1000
CAL_DEN = 1000
CAL_OFFSET = 0

SAMPLE_MS = 50        # one graph point every 50 ms
BATTERY_MS = 2000     # battery reading interval
BAT_EMPTY = 3300      # Li-ion: 0 % at 3.3 V ...
BAT_FULL = 4200       # ... 100 % at 4.2 V
BAT_NONE = 3000       # below this no usable cell is connected (board runs from USB;
                      # the battery pin then only sees the charger, ~2.6-2.7 V)
BAT_CHARGING = 4250   # above this a cell is charging from USB

# Graph area (the screen is 240 x 135)
GX = 30
GY = 50
GW = WIDTH - GX
GH = HEIGHT - GY - 1
GRID = rgb(40, 40, 40)
TRACE = display.GREEN
CURSOR = rgb(90, 90, 90)
LABEL = display.GRAY

pin = 36
hold = False
range_mv = 3300       # graph full scale: 3300, 1650 or 825 mV
mv = 0
low = 0               # min / max / average since the last reset
high = 0
avg = 0
count = 0
pos = 0               # graph column of the next sample
prev_y = -1
last_shown = -1


def to_y(v):
    # Millivolts to a graph row, clipped to the graph area
    y = GY + GH - v * GH // range_mv
    return max(GY, min(GY + GH, y))


def grid_row(r):
    return GY + GH * r // 4


def draw_volts(x, y, v):
    # Draws v millivolts as volts with two decimals: 3870 -> "3.87"
    w = display.number(x, y, v // 1000)
    display.text(x + w, y, '.')
    w += display.text_width('.')
    cents = v % 1000 // 10
    if cents < 10:
        w += display.number(x + w, y, 0)
    w += display.number(x + w, y, cents)
    return w


def draw_axes():
    display.fill_rect(0, GY - 2, WIDTH, HEIGHT - GY + 2, display.BLACK)
    display.font(1)
    r = 0
    while r <= 4:
        y = grid_row(r)
        display.hline(GX, y, GW, GRID)
        label = range_mv * (4 - r) // 4
        w = display.number_width(label)
        if r == 0:
            display.number(GX - 3 - w, y, label, LABEL)
        elif r == 2:
            display.number(GX - 3 - w, y - 4, label, LABEL)
        elif r == 4:
            display.number(GX - 3 - w, y - 8, label, LABEL)
        r += 1
    display.vline(GX - 1, GY, GH + 1, LABEL)


def clear_column(x):
    # Erases one graph column and redraws the grid dots in it
    display.vline(x, GY, GH + 1, display.BLACK)
    r = 0
    while r <= 4:
        display.pixel(x, grid_row(r), GRID)
        r += 1


def reset_graph():
    global pos, prev_y
    pos = 0
    prev_y = -1
    draw_axes()


def reset_stats():
    global low, high, avg, count
    low = 0
    high = 0
    avg = 0
    count = 0


def draw_header():
    display.fill_rect(0, 0, 150, 12, display.BLACK)
    display.font(1)
    display.text(2, 2, 'AIN GPIO', LABEL)
    display.number(2 + display.text_width('AIN GPIO'), 2, pin, LABEL)
    if hold:
        display.text(80, 2, 'HOLD', display.BLACK, display.YELLOW)


def draw_value(raw):
    display.fill_rect(0, 14, 150, 22, display.BLACK)
    display.font(3)
    if raw >= 4095:
        display.text(2, 15, '> 3.1 V', display.RED)
    elif raw == 0:
        display.text(2, 15, '< 150 mV', display.YELLOW)
    else:
        w = display.number(2, 15, mv, display.WHITE)
        display.font(2)
        display.text(8 + w, 21, 'mV', LABEL)


def draw_stats():
    display.fill_rect(0, 37, WIDTH, 10, display.BLACK)
    if count == 0:
        return
    display.font(1)
    x = 2
    display.text(x, 38, 'min', LABEL)
    x += display.text_width('min ')
    x += display.number(x, 38, low) + 8
    display.text(x, 38, 'max', LABEL)
    x += display.text_width('max ')
    x += display.number(x, 38, high) + 8
    display.text(x, 38, 'avg', LABEL)
    x += display.text_width('avg ')
    display.number(x, 38, avg)


def draw_battery():
    bat = battery_mv(8)
    x = WIDTH - 66
    display.fill_rect(x, 1, 66, 11, display.BLACK)
    display.fill_rect(150, 16, WIDTH - 150, 18, display.BLACK)
    display.font(1)
    if bat < BAT_NONE:
        # No battery: a grey empty icon and "USB" instead of a misleading 0 %
        display.rect(x, 1, 26, 11, display.GRAY)
        display.fill_rect(x + 26, 4, 2, 5, display.GRAY)
        display.text(x + 32, 3, 'USB', display.CYAN)
        display.font(2)
        w = display.text_width('no battery')
        display.text(WIDTH - 2 - w, 18, 'no battery', LABEL)
        return

    pct = (bat - BAT_EMPTY) * 100 // (BAT_FULL - BAT_EMPTY)
    pct = max(0, min(100, pct))
    if pct > 50:
        color = display.GREEN
    elif pct > 20:
        color = display.YELLOW
    else:
        color = display.RED
    # Icon: body 26 x 11 with a nub, filled by the charge level
    display.rect(x, 1, 26, 11, display.WHITE)
    display.fill_rect(x + 26, 4, 2, 5, display.WHITE)
    display.fill_rect(x + 2, 3, 22 * pct // 100, 7, color)
    if bat > BAT_CHARGING:
        display.text(x + 32, 3, 'CHG', display.CYAN)
    else:
        w = display.number(x + 32, 3, pct, color)
        display.text(x + 32 + w, 3, '%', color)
    # Battery voltage, right-aligned under the icon
    display.font(2)
    x = WIDTH - 2 - display.text_width('0.00 V')
    w = draw_volts(x, 18, bat)
    display.text(x + w, 18, ' V', LABEL)


def sample():
    # Reads the input, updates the statistics, value and graph
    global mv, low, high, avg, count, pos, prev_y, last_shown
    raw = adc_read(pin)
    mv = adc_mv(pin, 16) * CAL_NUM // CAL_DEN + CAL_OFFSET
    if count == 0:
        low = mv
        high = mv
        avg = mv
    low = min(low, mv)
    high = max(high, mv)
    avg += (mv - avg) // 8        # moving average
    count += 1
    if abs(mv - last_shown) > 2 or raw == 0 or raw >= 4095:
        draw_value(raw)
        last_shown = mv
    if count % 5 == 0:
        draw_stats()

    # Sweep graph: draw the new segment, erase the column ahead (scope style)
    x = GX + pos
    y = to_y(mv)
    clear_column(x)
    if prev_y >= 0:
        display.line(x - 1, prev_y, x, y, TRACE)
    else:
        display.pixel(x, y, TRACE)
    prev_y = y
    pos += 1
    if pos >= GW:
        pos = 0
        prev_y = -1
    display.vline(GX + pos, GY, GH + 1, CURSOR)


def handle(event):
    global pin, hold, range_mv
    if event == buttons.LEFT:
        if pin == 36:
            pin = 39
        else:
            pin = 36
        reset_stats()
        reset_graph()
        draw_header()
    elif event == buttons.LEFT_LONG:
        reset_stats()
        draw_stats()
    elif event == buttons.RIGHT:
        hold = not hold
        draw_header()
    elif event == buttons.RIGHT_LONG:
        if range_mv == 3300:
            range_mv = 1650
        elif range_mv == 1650:
            range_mv = 825
        else:
            range_mv = 3300
        reset_graph()


display.fill(display.BLACK)
draw_header()
draw_axes()
draw_battery()
print('voltmeter: LEFT = channel (hold: reset stats), RIGHT = hold (hold: range)')

# --- main loop (tests replace everything below this line) ---
next_sample = time.ticks_ms()
next_battery = next_sample + BATTERY_MS
while True:
    handle(buttons.get())
    now = time.ticks_ms()
    if not hold and now >= next_sample:
        sample()
        next_sample = now + SAMPLE_MS
    if now >= next_battery:
        draw_battery()
        next_battery = now + BATTERY_MS
    time.sleep_ms(5)
