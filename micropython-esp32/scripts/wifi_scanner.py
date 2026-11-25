# Wi-Fi scanner for the LilyGO TTGO T-Display (FEATURES=tft,wifi): the
# networks around, strongest first, with their signal, channel and security.
#
#   LEFT  button (GPIO0):  next network           (hold: previous network)
#   RIGHT button (GPIO35): show / close details   (hold: scan again)
#
# Uses the native `display` and `buttons` modules and MicroPython's `network`.
# No print() while the UI is up: on the TTGO, printed text is also drawn on
# the screen (the console), which would write over the UI and scroll it away.
# Drawing calls no Python functions of its own (labels, bars and colors are
# worked out once per scan): with Wi-Fi the stack has room for only about 5
# nested Python calls, and a redraw is then at most 3 levels deep.
# Make it start at every boot:  tools/mpy.py save scripts/wifi_scanner.py
import display
import buttons
from display import WIDTH, HEIGHT, rgb

try:
    import network
except ImportError:
    # Firmware without Wi-Fi: say so on the screen (a traceback would end
    # at the REPL, which looks like a crash)
    print('wifi scanner: this firmware has no Wi-Fi; flash it with: make flash FEATURES=tft,wifi')
    display.fill(display.BLACK)
    display.font(2)
    display.text(8, 30, 'No Wi-Fi in this', display.RED)
    display.text(8, 50, 'firmware. Flash it with', display.RED)
    display.font(1)
    display.text(8, 85, 'make flash FEATURES=tft,wifi', display.YELLOW)
    while True:
        buttons.wait()

# Layout (screen is 240 x 135): a title line on top, 4 network rows below
TOP = 24
ROW_H = 27
ROWS = 4

ROW_BG = rgb(40, 40, 70)
SEL_BG = display.ORANGE
DIM = display.GRAY

# Security, by MicroPython's network.AUTH_* number
SECURITY = ['open', 'WEP', 'WPA', 'WPA2', 'WPA/2', 'WPA2-E', 'WPA3', 'WPA2/3']

wlan = network.WLAN(network.STA_IF)

# Scanner state: per network (strongest first) its RSSI, SSID text, BSSID,
# channel, security name, and, for drawing, the SSID shortened for a row and
# for the details, its signal bars and their color
nets = []
sel = 0         # highlighted network
first = 0       # network in the top row
details = False


def bars(rssi):
    # Signal bars 0-4, like a phone shows them
    if rssi >= -55: return 4
    if rssi >= -67: return 3
    if rssi >= -75: return 2
    if rssi >= -85: return 1
    return 0


def bar_color(n):
    if n >= 3: return display.GREEN
    if n == 2: return display.YELLOW
    if n == 1: return display.ORANGE
    return display.RED


def security(code):
    if code < len(SECURITY):
        return SECURITY[code]
    return '?'


def printable(raw):
    # The SSID as text the font can draw ('?' for other characters)
    if len(raw) == 0:
        return '<hidden>'
    try:
        text = raw.decode()
    except ValueError:
        return '?' * len(raw)
    return ''.join([c if ' ' <= c <= '~' else '?' for c in text])


def fit(text, width):
    # `text` shortened (with '..') to `width` pixels in the current font
    if display.text_width(text) <= width:
        return text
    while len(text) > 1 and display.text_width(text + '..') > width:
        text = text[:-1]
    return text + '..'


OFF = rgb(70, 70, 90)   # a bar that isn't lit


def draw_bars(x, y, n, height, color):
    # 4 bars growing to the right, the first `n` lit in `color`
    for i in range(4):
        h = (i + 1) * height // 4
        display.fill_rect(x + i * 6, y + height - h, 4, h, color if i < n else OFF)


def title(text, color):
    display.fill_rect(0, 0, WIDTH, TOP - 2, display.BLACK)
    display.font(2)
    display.text(4, 4, 'Wi-Fi', display.CYAN)
    display.font(1)
    w = display.text_width(text)
    display.text(WIDTH - 4 - w, 7, text, color)


def draw_row(i):
    # Row of network `i` (on screen, between `first` and `first + ROWS`)
    y = TOP + (i - first) * ROW_H
    bg = SEL_BG if i == sel else ROW_BG
    display.fill_rect(1, y + 1, WIDTH - 2, ROW_H - 2, bg)
    net = nets[i]
    rssi, channel, sec, n, color = net[0], net[3], net[4], net[7], net[8]
    row_label = net[5]
    for b in range(4):          # the signal bars (inline: no extra call level)
        h = (b + 1) * 14 // 4
        display.fill_rect(6 + b * 6, y + 20 - h, 4, h, color if b < n else OFF)
    display.font(2)
    display.text(34, y + 3, row_label, display.WHITE, bg)
    display.font(1)
    w = display.text_width('%d dBm' % rssi)
    display.text(WIDTH - 6 - w, y + 3, '%d dBm' % rssi, display.WHITE, bg)
    info = 'ch %d %s' % (channel, sec)
    w = display.text_width(info)
    display.text(WIDTH - 6 - w, y + 14, info, display.WHITE if i == sel else DIM, bg)


def draw_list():
    display.fill_rect(0, TOP, WIDTH, HEIGHT - TOP, display.BLACK)
    if len(nets) == 0:
        display.font(2)
        display.text(10, TOP + 30, 'No networks', display.GRAY)
        display.font(1)
        display.text(10, TOP + 55, 'hold RIGHT to scan again', display.GRAY)
        return
    i = first
    while i < len(nets) and i < first + ROWS:
        draw_row(i)
        i += 1
    if len(nets) > ROWS:
        # Scroll bar on the right edge
        top = TOP + first * (HEIGHT - TOP) // len(nets)
        h = max(6, ROWS * (HEIGHT - TOP) // len(nets))
        display.fill_rect(WIDTH - 2, TOP, 2, HEIGHT - TOP, display.BLACK)
        display.fill_rect(WIDTH - 2, top, 2, h, DIM)


def draw_details():
    net = nets[sel]
    rssi, bssid, channel, sec, n, color = net[0], net[2], net[3], net[4], net[7], net[8]
    long_label = net[6]
    display.fill_rect(0, TOP, WIDTH, HEIGHT - TOP, display.BLACK)
    display.fill_rect(1, TOP + 1, WIDTH - 2, HEIGHT - TOP - 2, ROW_BG)
    display.font(2)
    display.text(8, TOP + 6, long_label, display.WHITE, ROW_BG)
    # Big signal number and bars
    display.font(3)
    w = display.number(8, TOP + 28, rssi, color, ROW_BG)
    display.font(1)
    display.text(12 + w, TOP + 38, 'dBm', display.WHITE, ROW_BG)
    draw_bars(WIDTH - 36, TOP + 26, n, 24, color)
    # Quality bar
    q = max(0, min(100, 2 * (rssi + 100)))      # -100 dBm: 0 %, -50 dBm: 100 %
    display.rect(8, TOP + 56, 160, 8, DIM)
    display.fill_rect(9, TOP + 57, 158 * q // 100, 6, color)
    display.text(176, TOP + 56, '%d %%' % q, display.WHITE, ROW_BG)
    # Address, channel, security
    mac = ':'.join(['%02x' % b for b in bssid])
    display.text(8, TOP + 72, 'BSSID   ' + mac, display.WHITE, ROW_BG)
    display.text(8, TOP + 86, 'channel %d    %s' % (channel, sec), display.WHITE, ROW_BG)
    title('%d of %d' % (sel + 1, len(nets)), display.YELLOW)


def scan():
    global nets, sel, first
    title('scanning...', display.YELLOW)
    found = sorted(wlan.scan(), key=lambda n: -n[3])
    display.font(2)
    nets = []
    for ssid_raw, bssid, channel, rssi, sec, hidden in found:
        ssid = printable(ssid_raw)
        n = bars(rssi)
        nets.append((rssi, ssid, bssid, channel, security(sec),
                     fit(ssid, 136), fit(ssid, WIDTH - 16), n, bar_color(n)))
    sel = 0
    first = 0
    title('%d networks' % len(nets), display.GREEN)


def move(step):
    # Highlight the next / previous network, scrolling the list as needed
    global sel, first
    if len(nets) == 0:
        return
    old = sel
    sel = (sel + step) % len(nets)
    if details:
        draw_details()
        return
    if sel < first or sel >= first + ROWS:
        first = max(0, min(sel - (ROWS - 1 if step > 0 else 0), len(nets) - ROWS))
        if sel < first:
            first = sel
        draw_list()
    else:
        draw_row(old)
        draw_row(sel)


def rescan():
    global details
    details = False
    scan()
    draw_list()


# The banner first: display.fill() then clears it from the screen
print('wifi scanner: LEFT = next (hold: previous), RIGHT = details (hold: scan again)')
display.fill(display.BLACK)
wlan.active(True)
rescan()

# --- main loop (tests replace everything below this line) ---
while True:
    event = buttons.wait()
    if event == buttons.LEFT:
        move(1)
    elif event == buttons.LEFT_LONG:
        move(-1)
    elif event == buttons.RIGHT and len(nets) > 0:
        details = not details
        if details:
            draw_details()
        else:
            title('%d networks' % len(nets), display.GREEN)
            draw_list()
    elif event == buttons.RIGHT_LONG:
        rescan()
