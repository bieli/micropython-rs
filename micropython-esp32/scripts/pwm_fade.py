# Fade an LED up and down with PWM (duty in percent)
LED_PIN = 2
duty = 0
step = 5
rounds = 0
while rounds < 2:
    pwm(LED_PIN, 1000, duty)
    sleep_ms(20)
    duty += step
    if duty >= 100 or duty <= 0:
        step = -step
        if duty <= 0:
            rounds += 1
pwm_stop(LED_PIN)
print('fade done')

# Servo pulse: 50 Hz, 7.5% duty = 1.5 ms = centre position
pwm(25, 50, 7)
sleep_ms(500)
pwm_stop(25)
print('servo pulse done')
