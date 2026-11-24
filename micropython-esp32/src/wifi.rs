//! Wi-Fi station for MicroPython's `network.WLAN` (the `wifi` feature).
//!
//! Started the first time a script activates it (`wlan.active(True)`), like
//! MicroPython: the esp-rtos scheduler (the radio driver runs its own tasks),
//! esp-radio's driver, and a smoltcp interface with a DHCP client. Scripts
//! then use flat functions (`wifi_connect`, `wifi_status`, ...), which the
//! frozen `network` module (modules/network.py) wraps in the `WLAN` class.
//!
//! There is no background network task: the interface is polled whenever a
//! script asks (`status()`, `isconnected()`, `ifconfig()`) and while it
//! sleeps (`sleep_ms`), which is when a MicroPython script waits for a
//! connection anyway.

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use core::sync::atomic::{AtomicU8, Ordering};

use esp_hal::peripherals::{TIMG1, WIFI};
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::event::{self, EventExt};
use esp_radio::wifi::{
    AuthMethod, ClientConfig, ModeConfig, ScanConfig, ScanTypeConfig, WifiController, WifiDevice,
};
use micropython_rs::Error;
use smoltcp::iface::{Config as IfaceConfig, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::dhcpv4;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address, Ipv4Cidr};

/// MicroPython's `network.STAT_*` values (its ESP32 port: the reasons of a
/// failed connection are the driver's disconnect reasons).
pub const STAT_IDLE: i64 = 1000;
pub const STAT_CONNECTING: i64 = 1001;
pub const STAT_GOT_IP: i64 = 1010;
pub const STAT_BEACON_TIMEOUT: i64 = 200;
pub const STAT_NO_AP_FOUND: i64 = 201;
pub const STAT_WRONG_PASSWORD: i64 = 202;
pub const STAT_ASSOC_FAIL: i64 = 203;
pub const STAT_HANDSHAKE_TIMEOUT: i64 = 204;

/// The reason of the last disconnection (0: none), set by the driver's event
/// handler.
static LAST_REASON: AtomicU8 = AtomicU8::new(0);

/// The peripherals Wi-Fi needs, kept until a script activates it.
pub struct WifiParts {
    pub wifi: WIFI<'static>,
    pub timg1: TIMG1<'static>,
}

/// The running station.
pub struct Station {
    controller: WifiController<'static>,
    device: WifiDevice<'static>,
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: SocketHandle,
    /// Address and netmask, gateway, DNS server from DHCP.
    ip: Option<(Ipv4Cidr, Option<Ipv4Address>, Option<Ipv4Address>)>,
    /// `connect()` was called (and not `disconnect()` since).
    connecting: bool,
    active: bool,
}

fn now() -> smoltcp::time::Instant {
    let us = esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros();
    smoltcp::time::Instant::from_micros(us as i64)
}

fn io<T>(_: T) -> Error {
    Error::IoError
}

impl Station {
    /// Starts the scheduler, the radio and the station interface.
    pub fn start(parts: WifiParts) -> Result<Self, Error> {
        // The radio driver's tasks run on esp-rtos (TIMG0 is esp-hal's clock)
        let timg1 = TimerGroup::new(parts.timg1);
        esp_rtos::start(timg1.timer0);
        let radio = esp_radio::init().map_err(io)?;
        // The driver borrows its controller for as long as Wi-Fi runs
        let radio: &'static _ = Box::leak(Box::new(radio));
        let (mut controller, interfaces) =
            esp_radio::wifi::new(radio, parts.wifi, Default::default()).map_err(io)?;
        controller
            .set_config(&ModeConfig::Client(ClientConfig::default()))
            .map_err(io)?;
        event::StaDisconnected::update_handler(|event| {
            LAST_REASON.store(event.reason(), Ordering::Relaxed);
        });

        let mut device = interfaces.sta;
        let mac = EthernetAddress(device.mac_address());
        let iface = Interface::new(
            IfaceConfig::new(HardwareAddress::Ethernet(mac)),
            &mut device,
            now(),
        );
        let storage: &'static mut [SocketStorage<'static>; 4] =
            Box::leak(Box::new(Default::default()));
        let mut sockets = SocketSet::new(&mut storage[..]);
        let dhcp = sockets.add(dhcpv4::Socket::new());
        let mut station = Station {
            controller,
            device,
            iface,
            sockets,
            dhcp,
            ip: None,
            connecting: false,
            active: false,
        };
        station.set_active(true)?;
        Ok(station)
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// `wlan.active(on)`: starts or stops the radio.
    pub fn set_active(&mut self, on: bool) -> Result<(), Error> {
        if on == self.active {
            return Ok(());
        }
        if on {
            self.controller.start().map_err(io)?;
            // Not blocking: wait (up to 2 s) until the driver has started
            let start = esp_hal::time::Instant::now();
            while !self.controller.is_started().unwrap_or(false) {
                if start.elapsed().as_millis() > 2000 {
                    return Err(Error::IoError);
                }
            }
        } else {
            self.forget_ip();
            self.connecting = false;
            self.controller.stop().map_err(io)?;
        }
        self.active = on;
        Ok(())
    }

    fn require_active(&self) -> Result<(), Error> {
        if self.active {
            Ok(())
        } else {
            Err(Error::IoError) // like MicroPython: "Wifi Not Started"
        }
    }

    /// `wlan.connect(ssid, key)`: starts connecting (doesn't wait).
    pub fn connect(&mut self, ssid: &str, key: &str) -> Result<(), Error> {
        self.require_active()?;
        if self.connecting {
            let _ = self.controller.disconnect();
        }
        self.forget_ip();
        // The minimum security accepted (WPA2 also accepts WPA3 networks)
        let auth = if key.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::Wpa2Personal
        };
        let config = ClientConfig::default()
            .with_ssid(String::from(ssid))
            .with_password(String::from(key))
            .with_auth_method(auth);
        self.controller
            .set_config(&ModeConfig::Client(config))
            .map_err(|_| Error::ValueError)?;
        LAST_REASON.store(0, Ordering::Relaxed);
        self.controller.connect().map_err(io)?;
        self.connecting = true;
        Ok(())
    }

    /// `wlan.disconnect()`.
    pub fn disconnect(&mut self) -> Result<(), Error> {
        self.connecting = false;
        self.forget_ip();
        if self.active {
            let _ = self.controller.disconnect();
        }
        Ok(())
    }

    fn forget_ip(&mut self) {
        if self.ip.take().is_some() {
            self.iface.update_ip_addrs(|addrs| addrs.clear());
            self.iface.routes_mut().remove_default_ipv4_route();
        }
        self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).reset();
    }

    /// Processes received packets and timers: DHCP gets its address here.
    pub fn poll(&mut self) {
        if !self.active {
            return;
        }
        let associated = self.controller.is_connected().unwrap_or(false);
        if !associated {
            if self.ip.is_some() {
                self.forget_ip();
            }
            return;
        }
        self.iface.poll(now(), &mut self.device, &mut self.sockets);
        match self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).poll() {
            Some(dhcpv4::Event::Configured(config)) => {
                let address = config.address;
                self.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    let _ = addrs.push(IpCidr::Ipv4(address));
                });
                if let Some(router) = config.router {
                    let _ = self.iface.routes_mut().add_default_ipv4_route(router);
                } else {
                    self.iface.routes_mut().remove_default_ipv4_route();
                }
                self.ip = Some((address, config.router, config.dns_servers.first().copied()));
            }
            Some(dhcpv4::Event::Deconfigured) => {
                self.ip = None;
                self.iface.update_ip_addrs(|addrs| addrs.clear());
                self.iface.routes_mut().remove_default_ipv4_route();
            }
            None => {}
        }
    }

    /// `wlan.status()`, with MicroPython's codes.
    pub fn status(&mut self) -> i64 {
        self.poll();
        if !self.active || !self.connecting {
            return STAT_IDLE;
        }
        if self.ip.is_some() {
            return STAT_GOT_IP;
        }
        if self.controller.is_connected().unwrap_or(false) {
            return STAT_CONNECTING; // associated, waiting for DHCP
        }
        match LAST_REASON.load(Ordering::Relaxed) {
            0 => STAT_CONNECTING,
            // The driver's reasons for a failed connection
            201 => STAT_NO_AP_FOUND,
            2 | 15 | 202 | 204 => STAT_WRONG_PASSWORD, // auth expired / 4-way handshake / auth fail
            200 => STAT_BEACON_TIMEOUT,
            203 => STAT_ASSOC_FAIL,
            _ => STAT_CONNECTING, // the driver keeps trying
        }
    }

    /// `wlan.ifconfig()`: address, netmask, gateway and DNS as 16 bytes.
    pub fn ifconfig(&mut self, out: &mut [u8]) -> usize {
        self.poll();
        let zero = Ipv4Address::new(0, 0, 0, 0);
        let (ip, mask, gw, dns) = match self.ip {
            Some((cidr, gw, dns)) => (
                cidr.address(),
                cidr.netmask(),
                gw.unwrap_or(zero),
                dns.unwrap_or(zero),
            ),
            None => (zero, zero, zero, zero),
        };
        for (i, a) in [ip, mask, gw, dns].iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&a.octets());
        }
        16
    }

    /// `wlan.scan()`: one record per access point:
    /// `len, ssid[len], bssid[6], channel, rssi (i8), authmode`.
    pub fn scan(&mut self, out: &mut [u8]) -> Result<usize, Error> {
        self.require_active()?;
        // Up to 120 ms per channel, like ESP-IDF's default (which MicroPython
        // uses): the driver's 10-20 ms miss access points that answer slowly
        let scan_type = ScanTypeConfig::Active {
            min: core::time::Duration::from_millis(10),
            max: core::time::Duration::from_millis(120),
        };
        let found = self
            .controller
            .scan_with_config(ScanConfig::default().with_scan_type(scan_type))
            .map_err(io)?;
        let mut n = 0;
        for ap in &found {
            let ssid = ap.ssid.as_bytes();
            let len = ssid.len().min(32);
            let record = 1 + len + 6 + 3;
            if n + record > out.len() {
                break;
            }
            out[n] = len as u8;
            out[n + 1..n + 1 + len].copy_from_slice(&ssid[..len]);
            let at = n + 1 + len;
            out[at..at + 6].copy_from_slice(&ap.bssid);
            out[at + 6] = ap.channel;
            out[at + 7] = ap.signal_strength as u8;
            out[at + 8] = auth_code(ap.auth_method);
            n += record;
        }
        Ok(n)
    }

    /// `wlan.status('rssi')`.
    pub fn rssi(&self) -> Result<i64, Error> {
        self.controller.rssi().map(|r| r as i64).map_err(io)
    }
}

/// MicroPython's `network.AUTH_*` value of an access point's security.
fn auth_code(auth: Option<AuthMethod>) -> u8 {
    match auth {
        None | Some(AuthMethod::None) => 0,
        Some(AuthMethod::Wep) => 1,
        Some(AuthMethod::Wpa) => 2,
        Some(AuthMethod::Wpa2Personal) => 3,
        Some(AuthMethod::WpaWpa2Personal) => 4,
        Some(AuthMethod::Wpa2Enterprise) => 5,
        Some(AuthMethod::Wpa3Personal) => 6,
        Some(AuthMethod::Wpa2Wpa3Personal) => 7,
        Some(_) => 8,
    }
}

/// The station's MAC address (from eFuse; also before Wi-Fi starts).
pub fn sta_mac() -> [u8; 6] {
    esp_radio::wifi::sta_mac()
}
