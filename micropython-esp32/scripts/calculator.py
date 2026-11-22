# Two-button integer calculator for the LilyGO TTGO T-Display (FEATURES=tft).
#
#   LEFT  button (GPIO0):  move to the next key        (hold: previous key)
#   RIGHT button (GPIO35): press the highlighted key   (hold: delete last digit)
#
# Uses the native `display` and `buttons` modules (written in Rust).
# Make it start at every boot:  tools/mpy.py save scripts/calculator.py
import display
import buttons
from display import WIDTH, rgb

# Layout: the result line on top, a 4 x 4 keypad below (screen is 240 x 135)
#    7 8 9 /
#    4 5 6 *
#    1 2 3 -
#    C 0 = +
COLS = 4
KEYS = 16
KEY_W = 60
KEY_H = 24
TOP = 38

# Key codes: 0-9 are digits
ADD = 10
SUB = 11
MUL = 12
DIV = 13
EQ = 14
CLR = 15

MAX_DIGITS = 9
LIMIT = 999999999999    # larger results show "Error"

KEY_BG = rgb(40, 40, 70)
OP_BG = rgb(80, 50, 20)
SEL_BG = display.ORANGE

# Calculator state
value = 0      # number on the display
acc = 0        # left operand of the pending operation
op = 0         # pending operation (0 = none)
fresh = True   # the next digit starts a new number
error = False
sel = 0        # highlighted key


def key_code(i):
    row = i // COLS
    col = i % COLS
    if col == 3:
        if row == 0: return DIV
        if row == 1: return MUL
        if row == 2: return SUB
        return ADD
    if row == 3:
        if col == 0: return CLR
        if col == 1: return 0
        return EQ
    return 7 - 3 * row + col


def label(code):
    if code == ADD: return '+'
    if code == SUB: return '-'
    if code == MUL: return '*'
    if code == DIV: return '/'
    if code == EQ: return '='
    return 'C'


def draw_key(i, selected):
    x = i % COLS * KEY_W
    y = TOP + i // COLS * KEY_H
    code = key_code(i)
    if selected:
        bg = SEL_BG
    elif code >= ADD:
        bg = OP_BG
    else:
        bg = KEY_BG
    display.fill_rect(x + 1, y + 1, KEY_W - 2, KEY_H - 2, bg)
    display.font(2)
    if code < ADD:
        w = display.number_width(code)
        display.number(x + (KEY_W - w) // 2, y + 6, code, display.WHITE, bg)
    else:
        w = display.text_width(label(code))
        display.text(x + (KEY_W - w) // 2, y + 6, label(code), display.WHITE, bg)


def show():
    display.fill_rect(0, 0, WIDTH, TOP - 2, display.BLACK)
    display.font(1)
    if op != 0:
        # The pending operation, small, top left: "12 +"
        w = display.number(4, 4, acc, display.GRAY)
        display.text(10 + w, 4, label(op), display.YELLOW)
    display.font(3)
    if error:
        w = display.text_width('Error')
        display.text(WIDTH - 6 - w, 14, 'Error', display.RED)
    else:
        w = display.number_width(value)
        display.number(WIDTH - 6 - w, 14, value, display.WHITE)


def compute(a, b, o):
    # Returns a o b, or sets `error` (division by zero, too large)
    global error
    if o == ADD:
        r = a + b
    elif o == SUB:
        r = a - b
    elif o == MUL:
        if a != 0 and abs(b) > LIMIT // abs(a):
            error = True
            return 0
        r = a * b
    else:
        if b == 0:
            error = True
            return 0
        # Like a pocket calculator, round toward zero: -7 / 2 = -3
        r = abs(a) // abs(b)
        if (a < 0) != (b < 0):
            r = -r
    if abs(r) > LIMIT:
        error = True
        return 0
    return r


def press(i):
    global value, acc, op, fresh, error
    code = key_code(i)
    if code == CLR:
        value = 0; acc = 0; op = 0; fresh = True; error = False
    elif error:
        return              # only C clears an error
    elif code < ADD:
        if fresh:
            value = 0
            fresh = False
        if value < 10 ** (MAX_DIGITS - 1):
            value = value * 10 + code
    elif code == EQ:
        if op != 0:
            value = compute(acc, value, op)
            op = 0
        fresh = True
    else:
        # An operator: finish the pending one first ("2 + 3 *" shows 5)
        if op != 0 and not fresh:
            value = compute(acc, value, op)
        acc = value
        op = code
        fresh = True
    show()


def backspace():
    global value
    if not fresh and not error:
        value = value // 10
        show()


def move(step):
    global sel
    draw_key(sel, False)
    sel = (sel + step) % KEYS
    draw_key(sel, True)


display.fill(display.BLACK)
i = 0
while i < KEYS:
    draw_key(i, i == sel)
    i += 1
show()
print('calculator: LEFT = next key (hold: back), RIGHT = press (hold: delete)')

# --- main loop (tests replace everything below this line) ---
while True:
    event = buttons.wait()
    if event == buttons.LEFT:
        move(1)
    elif event == buttons.LEFT_LONG:
        move(-1)
    elif event == buttons.RIGHT:
        press(sel)
    elif event == buttons.RIGHT_LONG:
        backspace()
