# Drawing with the native `display` module (TTGO T-Display, FEATURES=tft).
# The module is written in Rust (src/display_module.rs) and imported like a
# MicroPython module. Each step prints how long it took.
import display
import time
from display import WIDTH, HEIGHT, rgb

def step(name, start):
    print(name, time.ticks_diff(time.ticks_ms(), start), 'ms')
    return time.ticks_ms()

t = time.ticks_ms()
display.fill(display.BLACK)
t = step('fill', t)

# Frame and diagonals
display.rect(0, 0, WIDTH, HEIGHT, display.WHITE)
display.line(0, 0, WIDTH - 1, HEIGHT - 1, display.RED)
display.line(0, HEIGHT - 1, WIDTH - 1, 0, display.GREEN)
t = step('rect + 2 lines', t)

# A fan of lines from the bottom-left corner
x = 0
while x < WIDTH:
    display.line(0, HEIGHT - 1, x, 0, rgb(x, 255 - x, 128))
    x += 12
t = step('20 lines', t)

# Filled and outlined shapes
display.fill_rect(10, 10, 60, 35, display.BLUE)
display.fill_circle(120, 67, 30, display.ORANGE)
display.circle(120, 67, 40, display.YELLOW)
display.fill_triangle(170, 120, 230, 120, 200, 70, display.MAGENTA)
display.triangle(170, 120, 230, 120, 200, 70, display.WHITE)
t = step('shapes', t)

# Pixels: a dotted horizontal line
x = 10
while x < 110:
    display.pixel(x, 60, display.CYAN)
    x += 2
display.hline(10, 64, 100, display.GRAY)
display.vline(115, 10, 50, display.GRAY)
t = step('50 pixels + h/vline', t)

display.text(8, 120, 'import display', display.YELLOW)
display.text(8, 50, 'Rust', display.WHITE, display.BLUE)
step('text', t)
