# The board as a Wi-Fi access point (firmware built with FEATURES=wifi, e.g.
# `make flash FEATURES=tft,wifi`): phones and laptops join it and get an
# address from its DHCP server (192.168.4.2 and up); the board is 192.168.4.1.
import network
import time

ap = network.WLAN(network.AP_IF)
ap.config(ssid='my-board', key='change-me-123')    # WPA2; no key: an open network
ap.active(True)
print('access point', ap.config('ssid'), 'at', ap.ifconfig()[0], 'channel', ap.config('channel'))

known = []
while True:
    devices = [':'.join('%02x' % b for b in entry[0]) for entry in ap.status('stations')]
    for d in devices:
        if d not in known:
            print('joined:', d)
    for d in known:
        if d not in devices:
            print('left:', d)
    known = devices
    time.sleep_ms(500)                              # the DHCP server answers meanwhile
