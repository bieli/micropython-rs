//! The firmware's frozen `network` module (micropython-esp32/modules/network.py)
//! against a mock radio: MicroPython-style `network.WLAN` code must turn into
//! the right `wifi_*` calls and decode their results like MicroPython shows them.
#![cfg(all(feature = "modules", feature = "classes", feature = "format"))]

use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Interpreter, Output};
use std::fmt::Write;

const NETWORK: &str = include_str!("../micropython-esp32/modules/network.py");
const FROZEN: &[(&str, &str)] = &[("network", NETWORK)];

const FUNCTIONS: &[HwFn] = &[
    HwFn::with_args("wifi_active", 0, 1, "[on]", ""),
    HwFn::new("wifi_scan", "", "").returning_bytes(),
    HwFn::with_args("wifi_connect", 1, 2, "ssid, [key]", ""),
    HwFn::new("wifi_disconnect", "", ""),
    HwFn::new("wifi_status", "", ""),
    HwFn::new("wifi_ifconfig", "", "").returning_bytes(),
    HwFn::new("wifi_mac", "", "").returning_bytes(),
    HwFn::new("wifi_rssi", "", ""),
    HwFn::new("sleep_ms", "ms", ""),
];

/// A radio that connects after three status polls, unless the key is wrong.
#[derive(Default)]
struct MockRadio {
    log: String,
    active: bool,
    polls: u32,
    key_ok: bool,
    connecting: bool,
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
            // Two access points: "my-network" (channel 5, -75 dBm, WPA2) and "open" (channel 11, -88, open)
            "wifi_scan" => {
                let mut d = vec![10];
                d.extend_from_slice(b"my-network");
                d.extend_from_slice(&[1, 2, 3, 4, 5, 6, 5, (-75i8) as u8, 3]);
                d.push(4);
                d.extend_from_slice(b"open");
                d.extend_from_slice(&[9, 9, 9, 9, 9, 9, 11, (-88i8) as u8, 0]);
                d
            }
            "wifi_ifconfig" if self.connecting && self.key_ok => {
                vec![
                    192, 168, 0, 111, 255, 255, 255, 0, 192, 168, 0, 1, 8, 8, 8, 8,
                ]
            }
            "wifi_ifconfig" => vec![0; 16],
            "wifi_mac" => vec![0x88, 0x13, 0xbf, 0x97, 0xb9, 0xd0],
            _ => return Err(Error::UnknownIdentifier),
        };
        out[..data.len()].copy_from_slice(&data);
        Ok(data.len())
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
         try:\n    network.WLAN(network.AP_IF)\nexcept ValueError as e:\n    print('AP_IF:', e)\n",
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
         AP_IF: only network.STA_IF (no access point mode yet)\n"
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
