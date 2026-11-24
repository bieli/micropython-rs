# Connect to Wi-Fi with MicroPython's network module (firmware built with
# FEATURES=wifi, e.g. `make flash FEATURES=tft,wifi`). Put your network's
# name and password below.
import network
import time

SSID = 'your-network'
PASSWORD = 'your-password'

wlan = network.WLAN(network.STA_IF)
wlan.active(True)
print('MAC', ':'.join('%02x' % b for b in wlan.config('mac')))

for ssid, bssid, channel, rssi, security, hidden in sorted(wlan.scan(), key=lambda n: -n[3]):
    print('%-24s channel %2d  %4d dBm  security %d' % (ssid.decode(), channel, rssi, security))

if not wlan.isconnected():
    print('connecting to', SSID)
    wlan.connect(SSID, PASSWORD)
    start = time.ticks_ms()
    while not wlan.isconnected():
        status = wlan.status()
        if status == network.STAT_WRONG_PASSWORD:
            raise OSError('wrong password')
        if status == network.STAT_NO_AP_FOUND:
            raise OSError('network not found')
        if time.ticks_diff(time.ticks_ms(), start) > 20000:
            raise OSError('timeout, status %d' % status)
        time.sleep_ms(100)

ip, netmask, gateway, dns = wlan.ifconfig()
print('connected: IP', ip, 'gateway', gateway, 'DNS', dns, 'signal', wlan.status('rssi'), 'dBm')
