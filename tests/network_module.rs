//! The firmware's frozen `network` module (micropython-esp32/modules/network.py)
//! against a mock radio: MicroPython-style `network.WLAN` code must turn into
//! the right `wifi_*` calls and decode their results like MicroPython shows them.
#![cfg(all(feature = "modules", feature = "classes", feature = "format"))]

use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Interpreter, Output, Returns};
use std::fmt::Write;

const NETWORK: &str = include_str!("../micropython-esp32/modules/network.py");
const FROZEN: &[(&str, &str)] = &[("network", NETWORK)];

const FUNCTIONS: &[HwFn] = &[
    HwFn::with_args("wifi_active", 0, 1, "[on]", ""),
    HwFn::new("wifi_scan", "", "").returning_values(),
    HwFn::with_args("wifi_connect", 1, 2, "ssid, [key]", ""),
    HwFn::new("wifi_disconnect", "", ""),
    HwFn::new("wifi_status", "", ""),
    HwFn::new("wifi_ifconfig", "", "").returning_values(),
    HwFn::new("wifi_mac", "", "").returning_bytes(),
    HwFn::new("wifi_rssi", "", ""),
    HwFn::new("sleep_ms", "ms", ""),
    HwFn::with_args("wifi_ap_active", 0, 1, "[on]", ""),
    HwFn::with_args(
        "wifi_ap_config",
        6,
        6,
        "ssid, key, authmode, channel, hidden, max_clients",
        "",
    ),
    HwFn::new("wifi_ap_ifconfig", "", "").returning_values(),
    HwFn::new("wifi_ap_stations", "", "").returning_bytes(),
    HwFn::new("wifi_ap_mac", "", "").returning_bytes(),
];

/// A radio that connects after three status polls, unless the key is wrong.
#[derive(Default)]
struct MockRadio {
    log: String,
    active: bool,
    polls: u32,
    key_ok: bool,
    connecting: bool,
    ap_active: bool,
}

impl HardwareBridge for MockRadio {
    fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        if name != "wifi_status" && name != "sleep_ms" {
            let shown: Vec<String> = args
                .iter()
                .map(|a| match a {
                    Arg::Str(s) => format!("{:?}", s),
                    Arg::Int(n) => n.to_string(),
                    _ => "?".into(),
                })
                .collect();
            writeln!(self.log, "{}({})", name, shown.join(",")).unwrap();
        }
        match name {
            "wifi_active" => {
                if let Some(on) = args.first() {
                    self.active = on.int()? != 0;
                }
                Ok(Some(self.active as i64))
            }
            "wifi_connect" => {
                self.key_ok = args.get(1).map_or(Ok(""), |k| k.str())? == "secret123";
                self.connecting = true;
                self.polls = 0;
                Ok(None)
            }
            "wifi_disconnect" => {
                self.connecting = false;
                Ok(None)
            }
            "wifi_status" => {
                self.polls += 1;
                Ok(Some(match (self.connecting, self.key_ok, self.polls > 3) {
                    (false, _, _) => 1000,
                    (true, _, false) => 1001,
                    (true, true, true) => 1010,
                    (true, false, true) => 202,
                }))
            }
            "wifi_rssi" => Ok(Some(-67)),
            "wifi_ap_active" => {
                if let Some(on) = args.first() {
                    self.ap_active = on.int()? != 0;
                }
                Ok(Some(self.ap_active as i64))
            }
            "wifi_ap_config" => Ok(None),
            "sleep_ms" => Ok(None),
            _ => Err(Error::UnknownIdentifier),
        }
    }

    fn call_bytes(
        &mut self,
        name: &str,
        _args: &[Arg<'_>],
        out: &mut [u8],
    ) -> Result<usize, Error> {
        let data: Vec<u8> = match name {
            "wifi_mac" => vec![0x88, 0x13, 0xbf, 0x97, 0xb9, 0xd0],
            "wifi_ap_mac" => vec![0x8a, 0x13, 0xbf, 0x97, 0xb9, 0xd0],
            // One device joined while the access point is on
            "wifi_ap_stations" if self.ap_active => vec![2, 0, 0, 0, 0, 1],
            "wifi_ap_stations" => vec![],
            _ => return Err(Error::UnknownIdentifier),
        };
        out[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    fn call_values(
        &mut self,
        _module: Option<&str>,
        name: &str,
        _args: &[Arg<'_>],
        out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        let addresses = |out: &mut Returns<'_>, a: [&str; 4]| -> Result<(), Error> {
            out.begin_tuple()?;
            for s in a {
                out.str(s)?;
            }
            out.end()
        };
        match name {
            // Two access points: "my-network" (channel 5, -75 dBm, WPA2) and "open" (channel 11, -88, open)
            "wifi_scan" => {
                out.begin_list()?;
                for (ssid, bssid, channel, rssi, security) in [
                    (&b"my-network"[..], [1, 2, 3, 4, 5, 6], 5, -75, 3),
                    (&b"open"[..], [9; 6], 11, -88, 0),
                ] {
                    out.begin_tuple()?;
                    out.bytes(ssid)?;
                    out.bytes(&bssid)?;
                    out.int(channel)?;
                    out.int(rssi)?;
                    out.int(security)?;
                    out.bool(false)?;
                    out.end()?;
                }
                out.end()
            }
            "wifi_ifconfig" if self.connecting && self.key_ok => addresses(
                out,
                ["192.168.0.111", "255.255.255.0", "192.168.0.1", "8.8.8.8"],
            ),
            "wifi_ifconfig" => addresses(out, ["0.0.0.0"; 4]),
            "wifi_ap_ifconfig" => addresses(
                out,
                ["192.168.4.1", "255.255.255.0", "192.168.4.1", "0.0.0.0"],
            ),
            _ => Err(Error::UnknownIdentifier),
        }
    }

    fn functions(&self) -> &[HwFn] {
        FUNCTIONS
    }
}

struct Text(String);

impl Output for Text {
    fn write_str(&mut self, s: &str) {
        self.0.push_str(s);
    }
}

fn run(script: &str) -> (String, String) {
    let mut out = Text(String::new());
    let mut radio = MockRadio::default();
    let mut heap = vec![0u8; 32 * 1024];
    let mut vm = Interpreter::with_hardware(&mut out, &mut radio)
        .with_heap(&mut heap)
        .with_frozen_modules(FROZEN);
    if let Err(err) = vm.eval(script) {
        let mut message = Text(String::new());
        vm.write_exception(err, &mut message);
        panic!("{} at line {:?}", message.0, vm.error_line());
    }
    let _ = vm; // ends the borrows of `out` and `radio`
    (out.0, radio.log)
}

#[test]
fn connect_like_micropython() {
    let (out, log) = run(
        "import network\n\
         wlan = network.WLAN(network.STA_IF)\n\
         print(wlan.active(), wlan.active(True), wlan.isconnected(), wlan.status() == network.STAT_IDLE)\n\
         print(wlan.scan())\n\
         print(':'.join('%02x' % b for b in wlan.config('mac')))\n\
         wlan.connect('my-network', 'secret123')\n\
         while not wlan.isconnected():\n    sleep_ms(100)\n\
         print(wlan.ifconfig(), wlan.status('rssi'), wlan.config('ssid'), wlan.status() == network.STAT_GOT_IP)\n\
         wlan.disconnect()\n\
         print(wlan.isconnected(), wlan.ifconfig()[0])\n\
         wlan.connect('my-network', 'wrong')\n\
         while wlan.status() == network.STAT_CONNECTING:\n    sleep_ms(100)\n\
         print(wlan.status() == network.STAT_WRONG_PASSWORD, network.WLAN.IF_STA, network.AUTH_WPA2_PSK)\n\
         wlan.config(hostname='sensor-1')\n\
         print(network.hostname(), wlan.config('hostname'))\n\
         try:\n    network.WLAN(5)\nexcept ValueError as e:\n    print('WLAN(5):', e)\n",
    );
    assert_eq!(
        out,
        "False True False True\n\
         [(b'my-network', b'\\x01\\x02\\x03\\x04\\x05\\x06', 5, -75, 3, False), (b'open', b'\\t\\t\\t\\t\\t\\t', 11, -88, 0, False)]\n\
         88:13:bf:97:b9:d0\n\
         ('192.168.0.111', '255.255.255.0', '192.168.0.1', '8.8.8.8') -67 my-network True\n\
         False 0.0.0.0\n\
         True 0 3\n\
         sensor-1 sensor-1\n\
         WLAN(5): network.STA_IF or network.AP_IF\n"
    );
    assert!(
        log.starts_with("wifi_active()\nwifi_active(1)\n"),
        "{}",
        log
    );
    assert!(
        log.contains("wifi_connect(\"my-network\",\"secret123\")\n"),
        "{}",
        log
    );
    assert!(log.contains("wifi_disconnect()\n"), "{}", log);
}

#[test]
fn access_point_like_micropython() {
    let (out, log) = run(
        "import network\n\
         ap = network.WLAN(network.AP_IF)\n\
         print(ap.config('ssid'), ap.active(), ap.isconnected(), ap.status('stations'))\n\
         ap.config(ssid='my-board', key='12345678')\n\
         print(ap.config('authmode') == network.AUTH_WPA2_PSK, ap.config('channel'))\n\
         print(ap.active(True), ap.ifconfig(), ap.isconnected(), ap.status('stations'), ap.status())\n\
         ap.config(channel=6, hidden=True, max_clients=4)\n\
         ap.config(essid='open-board', password='', security=network.AUTH_OPEN)\n\
         print(ap.config('essid'), ap.config('hidden'), ap.config('max_clients'), ap.config('mac').hex())\n\
         try:\n    ap.connect('x', 'y')\nexcept OSError:\n    print('connect on AP_IF: OSError')\n\
         print(ap.active(False), ap.status('stations'))\n",
    );
    assert_eq!(
        out,
        "ESP_97B9D0 False False []\n\
         True 1\n\
         True ('192.168.4.1', '255.255.255.0', '192.168.4.1', '0.0.0.0') True [(b'\\x02\\x00\\x00\\x00\\x00\\x01',)] None\n\
         open-board True 4 8a13bf97b9d0\n\
         connect on AP_IF: OSError\n\
         False []\n"
    );
    // The settings go to the firmware when it's activated, and on a change
    // while it's on
    assert!(
        log.contains("wifi_ap_config(\"my-board\",\"12345678\",3,1,0,8)\nwifi_ap_active(1)\n"),
        "{}",
        log
    );
    assert!(
        log.contains("wifi_ap_config(\"my-board\",\"12345678\",3,6,1,4)\n"),
        "{}",
        log
    );
    assert!(
        log.contains("wifi_ap_config(\"open-board\",\"\",0,6,1,4)\n"),
        "{}",
        log
    );
}
