# network: MicroPython's network module (its ESP32 port's WLAN class), in
# Python on top of the firmware's wifi_* functions (the `wifi` feature).
#
#   import network, time
#   wlan = network.WLAN(network.STA_IF)
#   wlan.active(True)
#   wlan.connect('ssid', 'password')
#   while not wlan.isconnected():
#       time.sleep_ms(100)
#   print(wlan.ifconfig())
#
# Station mode only for now: WLAN(network.AP_IF) raises ValueError.
# connect() returns at once, like MicroPython; status() and isconnected()
# tell how it goes (the network is polled while the script sleeps).

STA_IF = 0
AP_IF = 1

STAT_IDLE = 1000
STAT_CONNECTING = 1001
STAT_GOT_IP = 1010
STAT_BEACON_TIMEOUT = 200
STAT_NO_AP_FOUND = 201
STAT_WRONG_PASSWORD = 202
STAT_ASSOC_FAIL = 203
STAT_HANDSHAKE_TIMEOUT = 204

AUTH_OPEN = 0
AUTH_WEP = 1
AUTH_WPA_PSK = 2
AUTH_WPA2_PSK = 3
AUTH_WPA_WPA2_PSK = 4
AUTH_WPA2_ENTERPRISE = 5
AUTH_WPA3_PSK = 6
AUTH_WPA2_WPA3_PSK = 7

_hostname = 'micropython-rs'


def hostname(name=None):
    global _hostname
    if name is None:
        return _hostname
    _hostname = name


def _ip(b, at):
    return '%d.%d.%d.%d' % (b[at], b[at + 1], b[at + 2], b[at + 3])


class WLAN:
    IF_STA = 0
    IF_AP = 1
    SEC_OPEN = 0
    SEC_WEP = 1
    SEC_WPA = 2
    SEC_WPA2 = 3
    SEC_WPA_WPA2 = 4
    SEC_WPA2_ENT = 5
    SEC_WPA3 = 6
    SEC_WPA2_WPA3 = 7

    def __init__(self, interface_id=0):
        if interface_id != STA_IF:
            raise ValueError('only network.STA_IF (no access point mode yet)')
        self._ssid = ''

    def active(self, is_active=None):
        if is_active is None:
            return bool(wifi_active())
        return bool(wifi_active(1 if is_active else 0))

    def connect(self, ssid=None, key=None, bssid=None):
        if ssid is None:
            ssid = self._ssid
        self._ssid = ssid
        wifi_connect(ssid, key or '')

    def disconnect(self):
        wifi_disconnect()

    def isconnected(self):
        return wifi_status() == STAT_GOT_IP

    def status(self, param=None):
        if param is None:
            return wifi_status()
        if param == 'rssi':
            return wifi_rssi()
        raise ValueError('unknown status param')

    def scan(self):
        # (ssid, bssid, channel, RSSI, security, hidden), as on MicroPython
        raw = wifi_scan()
        found = []
        i = 0
        while i < len(raw):
            n = raw[i]
            at = i + 1 + n
            rssi = raw[at + 7]
            found.append((raw[i + 1:at], raw[at:at + 6], raw[at + 6],
                          rssi - 256 if rssi > 127 else rssi, raw[at + 8], False))
            i = at + 9
        return found

    def ifconfig(self, config=None):
        if config is not None:
            raise ValueError('only DHCP (no static addresses yet)')
        b = wifi_ifconfig()
        return (_ip(b, 0), _ip(b, 4), _ip(b, 8), _ip(b, 12))

    def config(self, *args, **kwargs):
        # config('mac'), config('ssid'), config('hostname'); setting values
        # (hostname=, reconnects=, txpower=, pm=) is accepted and ignored,
        # except hostname, which network.hostname() then returns
        if kwargs:
            if 'hostname' in kwargs:
                hostname(kwargs['hostname'])
            return
        if len(args) != 1:
            raise TypeError('config() takes one parameter name')
        key = args[0]
        if key == 'mac':
            return wifi_mac()
        if key in ('ssid', 'essid'):
            return self._ssid
        if key in ('hostname', 'dhcp_hostname'):
            return hostname()
        raise ValueError('unknown config param')
