# Read an analog input (GPIO34) a few times, averaging 16 samples each time.
# With 11 dB attenuation, 0..4095 covers roughly 0..3.3 V.
PIN = 34

def read_avg(pin, samples):
    total = 0
    n = 0
    while n < samples:
        total += adc_read(pin)
        n += 1
    return total // samples

i = 0
while i < 5:
    raw = read_avg(PIN, 16)
    print('ADC', PIN, 'raw', raw, '~', raw * 3300 // 4095, 'mV')
    sleep_ms(500)
    i += 1
