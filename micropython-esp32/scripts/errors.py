# The hardware layer reports bad pins as errors instead of crashing the board
def try_pin(pin):
    pin_write(pin, 1)

try_pin(6)    # flash pin: not in the allowlist -> ValueError
