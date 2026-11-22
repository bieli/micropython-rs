# Count presses of the BOOT button (GPIO0, active low) for 5 seconds
BUTTON = 0
pin_mode(BUTTON, IN_PULLUP)
presses = 0
last = 1
start = ticks_ms()
while ticks_diff(ticks_ms(), start) < 5000:
    now = pin_read(BUTTON)
    if last == 1 and now == 0:
        presses += 1
        print('press', presses)
    last = now
    sleep_ms(20)    # also debounces the button
print('total presses:', presses)
