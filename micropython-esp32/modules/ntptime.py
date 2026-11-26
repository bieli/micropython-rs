# ntptime: MicroPython's ntptime module (firmware built with FEATURES=wifi).
# Built into the firmware (a frozen module).
#
#   import ntptime
#   ntptime.host = 'pool.ntp.org'   # the default; a name or an IP address
#   ntptime.settime()               # sets the board's clock (UTC); OSError on failure
#
# Needs a connected station (network.WLAN(network.STA_IF)); the name is
# looked up with the DNS server the router gave. time() is in seconds since
# 2000-01-01 UTC, MicroPython's ESP32 epoch.

host = 'pool.ntp.org'
timeout = 1


def time():
    return wifi_ntp(host, int(timeout * 1000)) // 1000000


def settime():
    us = wifi_ntp(host, int(timeout * 1000))
    rtc_set(us // 1000000, us % 1000000)
