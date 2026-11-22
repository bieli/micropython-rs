# Watchdog demo: sleep_ms() feeds it automatically, a busy loop doesn't.
# After the watchdog resets the board, the script runs again and sees why.
if reset_cause() == WDT_RESET:
    print('rebooted by the watchdog - demo done')
else:
    wdt_enable(1000)
    print('watchdog on (1 s); sleeping 3 s - no reset expected')
    sleep_ms(3000)
    print('still alive, now busy-looping without feeding...')
    while True:
        pass
