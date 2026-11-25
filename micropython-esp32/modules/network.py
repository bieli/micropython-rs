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
# The access point (both can be on at once):
#
#   ap = network.WLAN(network.AP_IF)
#   ap.config(ssid='my-board', key='12345678')   # WPA2 when there's a key
#   ap.active(True)                               # 192.168.4.1, DHCP for 192.168.4.2-9
#   print(ap.ifconfig(), ap.status('stations'))
#
# connect() returns at once, like MicroPython; status() and isconnected()
# tell how it goes (the network is polled while the script sleeps and at
# the >>> prompt).

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

# The access point's settings (one radio, shared by all WLAN(AP_IF) objects);
# ssid None: MicroPython's default, ESP_ and the end of the MAC address
_ap = {'ssid': None, 'key': '', 'authmode': 0, 'channel': 1, 'hidden': False, 'max_clients': 8}


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
        if interface_id not in (STA_IF, AP_IF):
            raise ValueError('network.STA_IF or network.AP_IF')
        self._if = interface_id
        self._ssid = ''

    def active(self, is_active=None):
        if self._if == AP_IF:
            if is_active:
                self._ap_apply()
            if is_active is None:
                return bool(wifi_ap_active())
            return bool(wifi_ap_active(1 if is_active else 0))
        if is_active is None:
            return bool(wifi_active())
        return bool(wifi_active(1 if is_active else 0))

    def _ap_apply(self):
        ssid = _ap['ssid']
        if ssid is None:
            ssid = 'ESP_' + ''.join('%02X' % b for b in wifi_ap_mac()[3:])
            _ap['ssid'] = ssid
        wifi_ap_config(ssid, _ap['key'], _ap['authmode'], _ap['channel'], 1 if _ap['hidden'] else 0, _ap['max_clients'])

    def connect(self, ssid=None, key=None, bssid=None):
        if self._if == AP_IF:
            raise OSError('connect() is for the station (STA_IF)')
        if ssid is None:
            ssid = self._ssid
        self._ssid = ssid
        wifi_connect(ssid, key or '')

    def disconnect(self):
        wifi_disconnect()

    def isconnected(self):
        # The access point: whether a device has joined it
        if self._if == AP_IF:
            return len(wifi_ap_stations()) > 0
        return wifi_status() == STAT_GOT_IP

    def status(self, param=None):
        if self._if == AP_IF:
            if param == 'stations':
                # (mac,) for each device joined, as on MicroPython
                macs = wifi_ap_stations()
                return [(macs[i:i + 6],) for i in range(0, len(macs), 6)]
            if param is None:
                return None
        elif param is None:
            return wifi_status()
        elif param == 'rssi':
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
            raise ValueError('no static addresses yet')
        b = wifi_ap_ifconfig() if self._if == AP_IF else wifi_ifconfig()
        return (_ip(b, 0), _ip(b, 4), _ip(b, 8), _ip(b, 12))

    def config(self, *args, **kwargs):
        # Station: config('mac'), config('ssid'), config('hostname'); setting
        # hostname= (reconnects=, txpower=, pm= are accepted and ignored).
        # Access point: ssid/essid, key/password, authmode/security, channel,
        # hidden, max_clients (a key without authmode means WPA2-PSK)
        if kwargs:
            if 'hostname' in kwargs:
                hostname(kwargs['hostname'])
            if self._if == AP_IF:
                names = {'essid': 'ssid', 'password': 'key', 'security': 'authmode'}
                for k in kwargs:
                    name = names.get(k, k)
                    if name in _ap:
                        _ap[name] = kwargs[k]
                if ('key' in kwargs or 'password' in kwargs) and not ('authmode' in kwargs or 'security' in kwargs):
                    _ap['authmode'] = AUTH_WPA2_PSK if _ap['key'] else AUTH_OPEN
                if wifi_ap_active():
                    self._ap_apply()
            return
        if len(args) != 1:
            raise TypeError('config() takes one parameter name')
        key = args[0]
        if key == 'mac':
            return wifi_ap_mac() if self._if == AP_IF else wifi_mac()
        if key in ('hostname', 'dhcp_hostname'):
            return hostname()
        if self._if == AP_IF:
            if _ap['ssid'] is None:
                _ap['ssid'] = 'ESP_' + ''.join('%02X' % b for b in wifi_ap_mac()[3:])
            name = {'essid': 'ssid', 'password': 'key', 'security': 'authmode'}.get(key, key)
            if name in _ap and name != 'key':
                return _ap[name]
        elif key in ('ssid', 'essid'):
            return self._ssid
        raise ValueError('unknown config param')
