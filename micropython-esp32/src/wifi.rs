//! Wi-Fi for MicroPython's `network.WLAN` (the `wifi` feature): a station
//! (client) and an access point, each on its own or both at once.
//!
//! Started the first time a script activates either (`wlan.active(True)`),
//! like MicroPython: the esp-rtos scheduler (the radio driver runs its own
//! tasks), esp-radio's driver, and two smoltcp interfaces: the station's with
//! a DHCP client, and the access point's at 192.168.4.1/24 with a small DHCP
//! server (`DhcpServer`) for the devices that join it. Scripts use flat
//! functions (`wifi_connect`, `wifi_ap_config`, ...), which the frozen
//! `network` module (modules/network.py) wraps in the `WLAN` class.
//!
//! There is no background network task: the interfaces are polled whenever
//! a script asks (`status()`, `isconnected()`, `ifconfig()`), while it sleeps
//! (`sleep_ms`), and while the REPL waits for input.

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU8, Ordering};

use critical_section::Mutex;
use esp_hal::peripherals::{TIMG1, WIFI};
use esp_hal::timer::timg::TimerGroup;
use esp_radio::wifi::event::{self, EventExt};
use esp_radio::wifi::{
    AccessPointConfig, AuthMethod, ClientConfig, InternalWifiError, ModeConfig, ScanConfig,
    ScanTypeConfig, WifiController, WifiDevice, WifiError,
};
use micropython_rs::{Error, Returns};
use smoltcp::iface::{Config as IfaceConfig, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::{dhcpv4, dns, udp};
use smoltcp::wire::{
    DnsQueryType, EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address,
    Ipv4Cidr,
};

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

/// The access point's address and network, as on MicroPython's ESP32 port.
pub const AP_IP: Ipv4Address = Ipv4Address::new(192, 168, 4, 1);
const AP_PREFIX: u8 = 24;
/// Devices on the access point at once (and DHCP leases: 192.168.4.2 up).
pub const MAX_AP_CLIENTS: usize = 8;

/// The reason of the last disconnection (0: none), set by the driver's event
/// handler.
static LAST_REASON: AtomicU8 = AtomicU8::new(0);

/// The devices joined to the access point (their MAC addresses), kept by the
/// driver's event handlers.
static AP_STATIONS: Mutex<RefCell<[Option<[u8; 6]>; MAX_AP_CLIENTS]>> =
    Mutex::new(RefCell::new([None; MAX_AP_CLIENTS]));

/// The peripherals Wi-Fi needs, kept until a script activates it.
pub struct WifiParts {
    pub wifi: WIFI<'static>,
    pub timg1: TIMG1<'static>,
    /// RISC-V (ESP32-C3): the scheduler switches tasks with software interrupt 0
    #[cfg(feature = "esp32c3")]
    pub sw_int0: esp_hal::interrupt::software::SoftwareInterrupt<'static, 0>,
}

/// One network interface: the radio's side and smoltcp's.
struct Net {
    device: WifiDevice<'static>,
    iface: Interface,
    sockets: SocketSet<'static>,
}

impl Net {
    fn new(mut device: WifiDevice<'static>) -> Self {
        let mac = EthernetAddress(device.mac_address());
        let iface = Interface::new(
            IfaceConfig::new(HardwareAddress::Ethernet(mac)),
            &mut device,
            now(),
        );
        let storage: &'static mut [SocketStorage<'static>; 4] =
            Box::leak(Box::new(Default::default()));
        Net {
            device,
            iface,
            sockets: SocketSet::new(&mut storage[..]),
        }
    }

    fn poll(&mut self) {
        self.iface.poll(now(), &mut self.device, &mut self.sockets);
    }
}

/// Wi-Fi: the controller, the station and the access point.
pub struct Wifi {
    controller: WifiController<'static>,
    sta: Net,
    dhcp: SocketHandle,
    /// The station's DNS resolver and NTP socket
    dns: SocketHandle,
    ntp: SocketHandle,
    ap: Net,
    dhcp_server: DhcpServer,
    sta_active: bool,
    ap_active: bool,
    started: bool,
    client: ClientConfig,
    ap_config: AccessPointConfig,
    /// The station's address and netmask, gateway, DNS server from DHCP.
    ip: Option<(Ipv4Cidr, Option<Ipv4Address>, Option<Ipv4Address>)>,
    /// `connect()` was called (and not `disconnect()` since).
    connecting: bool,
}

fn now() -> smoltcp::time::Instant {
    let us = esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros();
    smoltcp::time::Instant::from_micros(us as i64)
}

/// How long `scan()` waits for a connection in progress to finish.
const SCAN_WAIT_MS: u64 = 5000;

fn io<T>(_: T) -> Error {
    Error::IoError
}

impl Wifi {
    /// Starts the scheduler, the radio driver and both interfaces (the radio
    /// itself starts with the first `set_active` / `set_ap_active`).
    pub fn start(parts: WifiParts) -> Result<Self, Error> {
        // The radio driver's tasks run on esp-rtos (TIMG0 is esp-hal's clock)
        let timg1 = TimerGroup::new(parts.timg1);
        esp_rtos::start(
            timg1.timer0,
            #[cfg(feature = "esp32c3")]
            parts.sw_int0,
        );
        let radio = esp_radio::init().map_err(io)?;
        // The driver borrows its controller for as long as Wi-Fi runs
        let radio: &'static _ = Box::leak(Box::new(radio));
        // Fewer receive/transmit buffers than the driver's defaults (10 + 32 /
        // 32, about 1.6 KB each), sized for a microcontroller with a few
        // clients: with both interfaces busy, the default ones took the heap
        // to 64 of 64 KB
        let config = esp_radio::wifi::Config::default()
            .with_static_rx_buf_num(6)
            .with_dynamic_rx_buf_num(12)
            .with_dynamic_tx_buf_num(12)
            .with_rx_ba_win(4);
        let (controller, interfaces) =
            esp_radio::wifi::new(radio, parts.wifi, config).map_err(io)?;
        event::StaDisconnected::update_handler(|event| {
            LAST_REASON.store(event.reason(), Ordering::Relaxed);
        });
        event::ApStaConnected::update_handler(|event| {
            let mut mac = [0u8; 6];
            mac.copy_from_slice(&event.mac()[..6]);
            critical_section::with(|cs| {
                let mut stations = AP_STATIONS.borrow_ref_mut(cs);
                if !stations.contains(&Some(mac)) {
                    if let Some(free) = stations.iter_mut().find(|s| s.is_none()) {
                        *free = Some(mac);
                    }
                }
            });
        });
        event::ApStaDisconnected::update_handler(|event| {
            let mac = &event.mac()[..6];
            critical_section::with(|cs| {
                for s in AP_STATIONS.borrow_ref_mut(cs).iter_mut() {
                    if s.is_some_and(|m| m == mac) {
                        *s = None;
                    }
                }
            });
        });

        let mut sta = Net::new(interfaces.sta);
        let dhcp = sta.sockets.add(dhcpv4::Socket::new());
        let queries: &'static mut [Option<dns::DnsQuery>; 1] = Box::leak(Box::new([None]));
        let dns = sta
            .sockets
            .add(dns::Socket::new(&[FALLBACK_DNS.into()], &mut queries[..]));
        let ntp = sta.sockets.add(udp_socket(1, 128));
        let mut ap = Net::new(interfaces.ap);
        ap.iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(AP_IP.into(), AP_PREFIX));
        });
        let dhcp_server = DhcpServer::new(&mut ap.sockets);
        let mac = ap.device.mac_address();
        let ap_config = AccessPointConfig::default()
            .with_ssid(default_ap_ssid(mac))
            .with_max_connections(MAX_AP_CLIENTS as u16);
        Ok(Wifi {
            controller,
            sta,
            dhcp,
            dns,
            ntp,
            ap,
            dhcp_server,
            sta_active: false,
            ap_active: false,
            started: false,
            client: ClientConfig::default(),
            ap_config,
            ip: None,
            connecting: false,
        })
    }

    pub fn is_active(&self) -> bool {
        self.sta_active
    }

    pub fn is_ap_active(&self) -> bool {
        self.ap_active
    }

    /// The driver's configuration for the interfaces that are on.
    fn mode_config(&self) -> ModeConfig {
        match (self.sta_active, self.ap_active) {
            (true, true) => ModeConfig::ApSta(self.client.clone(), self.ap_config.clone()),
            (true, false) => ModeConfig::Client(self.client.clone()),
            (false, true) => ModeConfig::AccessPoint(self.ap_config.clone()),
            (false, false) => ModeConfig::None,
        }
    }

    /// Gives the driver the configuration of the interfaces that are on, and
    /// starts or stops the radio to match.
    fn apply(&mut self) -> Result<(), Error> {
        if !self.sta_active && !self.ap_active {
            if self.started {
                self.controller.stop().map_err(io)?;
                self.started = false;
            }
            return Ok(());
        }
        // A new mode (station, access point, both) needs the radio restarted
        if self.started {
            self.controller.stop().map_err(io)?;
            self.started = false;
        }
        self.controller
            .set_config(&self.mode_config())
            .map_err(|_| Error::ValueError)?;
        self.controller.start().map_err(io)?;
        // Not blocking: wait (up to 2 s) until the driver has started
        let start = esp_hal::time::Instant::now();
        while !self.controller.is_started().unwrap_or(false) {
            if start.elapsed().as_millis() > 2000 {
                return Err(Error::IoError);
            }
        }
        self.started = true;
        if self.sta_active && self.connecting {
            // The station reconnects after the restart
            LAST_REASON.store(0, Ordering::Relaxed);
            self.controller.connect().map_err(io)?;
        }
        Ok(())
    }

    /// `WLAN(STA_IF).active(on)`.
    pub fn set_active(&mut self, on: bool) -> Result<(), Error> {
        if on == self.sta_active {
            return Ok(());
        }
        if !on {
            self.forget_ip();
            self.connecting = false;
        }
        self.sta_active = on;
        self.apply()
    }

    /// `WLAN(AP_IF).active(on)`.
    pub fn set_ap_active(&mut self, on: bool) -> Result<(), Error> {
        if on == self.ap_active {
            return Ok(());
        }
        self.ap_active = on;
        if !on {
            critical_section::with(|cs| *AP_STATIONS.borrow_ref_mut(cs) = [None; MAX_AP_CLIENTS]);
            self.dhcp_server.leases = [None; MAX_AP_CLIENTS];
        }
        self.apply()
    }

    /// `WLAN(AP_IF).config(ssid=, key=, authmode=, channel=, hidden=,
    /// max_clients=)`: an `authmode` of MicroPython's `AUTH_*` values.
    pub fn configure_ap(
        &mut self,
        ssid: &str,
        key: &str,
        authmode: i64,
        channel: i64,
        hidden: bool,
        max_clients: i64,
    ) -> Result<(), Error> {
        let auth = match authmode {
            0 => AuthMethod::None,
            2 => AuthMethod::Wpa,
            3 => AuthMethod::Wpa2Personal,
            4 => AuthMethod::WpaWpa2Personal,
            6 => AuthMethod::Wpa3Personal,
            7 => AuthMethod::Wpa2Wpa3Personal,
            _ => return Err(Error::ValueError), // WEP and enterprise: not for an access point
        };
        if ssid.is_empty() || ssid.len() > 32 || !(1..=13).contains(&channel) {
            return Err(Error::ValueError);
        }
        // WPA needs a password of 8-63 characters, an open network none
        if auth != AuthMethod::None && !(8..=63).contains(&key.len()) {
            return Err(Error::ValueError);
        }
        let max = max_clients.clamp(1, MAX_AP_CLIENTS as i64) as u16;
        self.ap_config = AccessPointConfig::default()
            .with_ssid(String::from(ssid))
            .with_password(String::from(if auth == AuthMethod::None {
                ""
            } else {
                key
            }))
            .with_auth_method(auth)
            .with_channel(channel as u8)
            .with_ssid_hidden(hidden)
            .with_max_connections(max);
        if self.ap_active {
            self.apply()?;
        }
        Ok(())
    }

    fn require_sta(&self) -> Result<(), Error> {
        if self.sta_active {
            Ok(())
        } else {
            Err(Error::IoError) // like MicroPython: "Wifi Not Started"
        }
    }

    /// `wlan.connect(ssid, key)`: starts connecting (doesn't wait).
    pub fn connect(&mut self, ssid: &str, key: &str) -> Result<(), Error> {
        self.require_sta()?;
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
        self.client = ClientConfig::default()
            .with_ssid(String::from(ssid))
            .with_password(String::from(key))
            .with_auth_method(auth);
        self.controller
            .set_config(&self.mode_config())
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
        if self.sta_active {
            let _ = self.controller.disconnect();
        }
        Ok(())
    }

    fn forget_ip(&mut self) {
        if self.ip.take().is_some() {
            self.sta.iface.update_ip_addrs(|addrs| addrs.clear());
            self.sta.iface.routes_mut().remove_default_ipv4_route();
        }
        self.sta
            .sockets
            .get_mut::<dhcpv4::Socket>(self.dhcp)
            .reset();
    }

    /// Processes received packets and timers on both interfaces: the
    /// station's DHCP gets its address, and the access point's DHCP server
    /// answers here.
    pub fn poll(&mut self) {
        if self.ap_active {
            self.ap.poll();
            self.dhcp_server.serve(&mut self.ap.sockets);
            self.ap.poll(); // send the answers now
        }
        if !self.sta_active {
            return;
        }
        let associated = self.controller.is_connected().unwrap_or(false);
        if !associated {
            if self.ip.is_some() {
                self.forget_ip();
            }
            return;
        }
        self.sta.poll();
        match self.sta.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).poll() {
            Some(dhcpv4::Event::Configured(config)) => {
                let address = config.address;
                self.sta.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    let _ = addrs.push(IpCidr::Ipv4(address));
                });
                if let Some(router) = config.router {
                    let _ = self.sta.iface.routes_mut().add_default_ipv4_route(router);
                } else {
                    self.sta.iface.routes_mut().remove_default_ipv4_route();
                }
                self.ip = Some((address, config.router, config.dns_servers.first().copied()));
            }
            Some(dhcpv4::Event::Deconfigured) => {
                self.ip = None;
                self.sta.iface.update_ip_addrs(|addrs| addrs.clear());
                self.sta.iface.routes_mut().remove_default_ipv4_route();
            }
            None => {}
        }
    }

    /// `wlan.status()`, with MicroPython's codes.
    pub fn status(&mut self) -> i64 {
        self.poll();
        if !self.sta_active || !self.connecting {
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

    /// `WLAN(STA_IF).ifconfig()`: address, netmask, gateway and DNS as 16
    /// bytes.
    pub fn ifconfig(&mut self) -> [Ipv4Address; 4] {
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
        [ip, mask, gw, dns]
    }

    /// `WLAN(AP_IF).ifconfig()`: 192.168.4.1/24, the access point is also the
    /// gateway (there's no DNS server).
    pub fn ap_ifconfig(&mut self) -> [Ipv4Address; 4] {
        self.poll();
        let mask = Ipv4Cidr::new(AP_IP, AP_PREFIX).netmask();
        [AP_IP, mask, AP_IP, Ipv4Address::new(0, 0, 0, 0)]
    }

    /// `WLAN(AP_IF).status('stations')`: the MAC addresses of the devices
    /// joined, 6 bytes each.
    pub fn ap_stations(&mut self, out: &mut [u8]) -> usize {
        self.poll();
        let stations = critical_section::with(|cs| *AP_STATIONS.borrow_ref(cs));
        let mut n = 0;
        for mac in stations.iter().flatten() {
            out[n..n + 6].copy_from_slice(mac);
            n += 6;
        }
        n
    }

    /// `wlan.scan()`: one record per access point:
    /// `len, ssid[len], bssid[6], channel, rssi (i8), authmode`.
    /// `wlan.scan()`: a list of (ssid, bssid, channel, RSSI, security,
    /// hidden) tuples, as MicroPython gives them (ssid and bssid as bytes).
    pub fn scan(&mut self, out: &mut Returns<'_>) -> Result<(), Error> {
        self.require_sta()?;
        // Up to 120 ms per channel, like ESP-IDF's default (which MicroPython
        // uses): the driver's 10-20 ms miss access points that answer slowly
        let scan_type = ScanTypeConfig::Active {
            min: core::time::Duration::from_millis(10),
            max: core::time::Duration::from_millis(120),
        };
        let config = ScanConfig::default().with_scan_type(scan_type);
        // The driver doesn't scan while the station is connecting
        // (ESP_ERR_WIFI_STATE): after connect(), and after an access point
        // change restarts the radio and the station reconnects. Wait for it
        // (up to SCAN_WAIT_MS), keeping the network going, then scan.
        let start = esp_hal::time::Instant::now();
        let found = loop {
            match self.controller.scan_with_config(config) {
                Ok(found) => break found,
                Err(WifiError::InternalError(InternalWifiError::State))
                    if start.elapsed().as_millis() < SCAN_WAIT_MS =>
                {
                    let pause = esp_hal::time::Instant::now();
                    while pause.elapsed().as_millis() < 50 {
                        self.poll();
                    }
                }
                Err(e) => return Err(io(e)),
            }
        };
        out.begin_list()?;
        for ap in &found {
            out.begin_tuple()?;
            out.bytes(ap.ssid.as_bytes())?;
            out.bytes(&ap.bssid)?;
            out.int(ap.channel as i64)?;
            out.int(ap.signal_strength as i64)?;
            out.int(auth_code(ap.auth_method) as i64)?;
            out.bool(false)?;
            out.end()?;
        }
        out.end()
    }

    /// `wlan.status('rssi')`.
    pub fn rssi(&self) -> Result<i64, Error> {
        self.controller.rssi().map(|r| r as i64).map_err(io)
    }

    /// Polls the station until `done` gives a result, `timeout_ms` passes
    /// (`OSError`) or Ctrl-C.
    fn wait_for<T>(
        &mut self,
        timeout_ms: u64,
        mut done: impl FnMut(&mut Self) -> Option<Result<T, Error>>,
    ) -> Result<T, Error> {
        let start = esp_hal::time::Instant::now();
        loop {
            self.sta.poll();
            if let Some(result) = done(self) {
                return result;
            }
            if crate::ABORT.load(Ordering::Relaxed) {
                return Err(Error::Interrupted);
            }
            if start.elapsed().as_millis() > timeout_ms {
                return Err(Error::IoError); // timed out
            }
        }
    }

    /// The IPv4 address of `host`: a dotted address as it is, a name from
    /// the DNS server DHCP gave (or 8.8.8.8).
    pub fn resolve(&mut self, host: &str, timeout_ms: u64) -> Result<Ipv4Address, Error> {
        if let Ok(address) = host.parse::<Ipv4Address>() {
            return Ok(address);
        }
        if self.ip.is_none() {
            return Err(Error::IoError); // not connected
        }
        let server = self.ip.and_then(|(_, _, dns)| dns).unwrap_or(FALLBACK_DNS);
        let socket = self.sta.sockets.get_mut::<dns::Socket>(self.dns);
        socket.update_servers(&[server.into()]);
        let query = socket
            .start_query(self.sta.iface.context(), host, DnsQueryType::A)
            .map_err(|_| Error::ValueError)?;
        let dns_handle = self.dns;
        // A finished query (answered or failed) frees its slot by itself
        let mut finished = false;
        let result = self.wait_for(timeout_ms, |wifi| {
            let socket = wifi.sta.sockets.get_mut::<dns::Socket>(dns_handle);
            let result = socket.get_query_result(query);
            finished = !matches!(result, Err(dns::GetQueryResultError::Pending));
            match result {
                Ok(addresses) => Some(
                    addresses
                        .iter()
                        .map(|a| match a {
                            IpAddress::Ipv4(v4) => *v4,
                        })
                        .next()
                        .ok_or(Error::IoError),
                ),
                Err(dns::GetQueryResultError::Pending) => None,
                Err(dns::GetQueryResultError::Failed) => Some(Err(Error::IoError)),
            }
        });
        if !finished {
            // Timed out or Ctrl-C: the query keeps its slot until cancelled
            self.sta
                .sockets
                .get_mut::<dns::Socket>(self.dns)
                .cancel_query(query);
        }
        result
    }

    /// The time from an NTP server (SNTP, RFC 4330): microseconds since
    /// 2000-01-01 (UTC, MicroPython's ESP32 epoch), with half the round trip
    /// added.
    pub fn ntp(&mut self, host: &str, timeout_ms: u64) -> Result<i64, Error> {
        let server = self.resolve(host, timeout_ms)?;
        let socket = self.sta.sockets.get_mut::<udp::Socket>(self.ntp);
        if !socket.is_open() {
            socket.bind(NTP_LOCAL_PORT).map_err(io)?;
        }
        // Drop old answers
        while socket.recv().is_ok() {}
        let mut request = [0u8; 48];
        request[0] = 0x23; // version 4, client
        let sent_at = esp_hal::time::Instant::now();
        socket
            .send_slice(&request, IpEndpoint::new(server.into(), 123))
            .map_err(io)?;
        let ntp_handle = self.ntp;
        let reply = self.wait_for(timeout_ms, |wifi| {
            let socket = wifi.sta.sockets.get_mut::<udp::Socket>(ntp_handle);
            let mut reply = [0u8; 48];
            match socket.recv_slice(&mut reply) {
                Ok((n, _)) if n >= 48 => Some(Ok(reply)),
                _ => None,
            }
        })?;
        let round_trip_us = sent_at.elapsed().as_micros() as i64;
        // A server's answer (mode 4), synchronized (stratum 1-15)
        if reply[0] & 7 != 4 || !(1..=15).contains(&reply[1]) {
            return Err(Error::IoError);
        }
        let secs = u32::from_be_bytes([reply[40], reply[41], reply[42], reply[43]]) as i64;
        let frac = u32::from_be_bytes([reply[44], reply[45], reply[46], reply[47]]) as i64;
        // NTP counts from 1900; the era wraps in 2036 (then secs is small)
        let secs = if secs >= NTP_TO_2000 {
            secs
        } else {
            secs + (1 << 32)
        };
        Ok((secs - NTP_TO_2000) * 1_000_000 + (frac * 1_000_000 >> 32) + round_trip_us / 2)
    }
}

/// Seconds from 1900 (NTP's epoch) to 2000-01-01.
const NTP_TO_2000: i64 = 3_155_673_600;
const NTP_LOCAL_PORT: u16 = 50123;
/// The DNS server when DHCP didn't give one.
const FALLBACK_DNS: Ipv4Address = Ipv4Address::new(8, 8, 8, 8);

/// A UDP socket with room for `packets` of `bytes` each way.
fn udp_socket(packets: usize, bytes: usize) -> udp::Socket<'static> {
    let meta = || -> &'static mut [udp::PacketMetadata] {
        alloc::vec![udp::PacketMetadata::EMPTY; packets].leak()
    };
    let data = || -> &'static mut [u8] { alloc::vec![0u8; packets * bytes].leak() };
    udp::Socket::new(
        udp::PacketBuffer::new(meta(), data()),
        udp::PacketBuffer::new(meta(), data()),
    )
}

/// `ifconfig()`'s tuple: four dotted addresses.
pub fn write_addresses(out: &mut Returns<'_>, addresses: [Ipv4Address; 4]) -> Result<(), Error> {
    use core::fmt::Write;
    out.begin_tuple()?;
    for a in addresses {
        let mut text = crate::stack_text::<15>();
        let [b0, b1, b2, b3] = a.octets();
        let _ = write!(text, "{}.{}.{}.{}", b0, b1, b2, b3);
        out.str(text.as_str())?;
    }
    out.end()
}

/// MicroPython's default access point name: `ESP_` and the end of its MAC.
fn default_ap_ssid(mac: [u8; 6]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut ssid = String::from("ESP_");
    for b in &mac[3..] {
        ssid.push(HEX[(b >> 4) as usize] as char);
        ssid.push(HEX[(b & 15) as usize] as char);
    }
    ssid
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

/// The access point's MAC address.
pub fn ap_mac() -> [u8; 6] {
    esp_radio::wifi::ap_mac()
}

/// A small DHCP server for the access point (RFC 2131, what phones and
/// laptops need): DISCOVER gets an OFFER, REQUEST an ACK (or a NAK for an
/// address it didn't offer), RELEASE frees the address. Device `i` gets
/// 192.168.4.(2 + i), for 2 hours; the access point is the router.
struct DhcpServer {
    socket: SocketHandle,
    /// The MAC address that has each address.
    leases: [Option<[u8; 6]>; MAX_AP_CLIENTS],
}

const DHCP_DISCOVER: u8 = 1;
const DHCP_OFFER: u8 = 2;
const DHCP_REQUEST: u8 = 3;
const DHCP_ACK: u8 = 5;
const DHCP_NAK: u8 = 6;
const DHCP_RELEASE: u8 = 7;
const DHCP_MAGIC: [u8; 4] = [99, 130, 83, 99];
const LEASE_SECONDS: u32 = 7200;

impl DhcpServer {
    fn new(sockets: &mut SocketSet<'static>) -> Self {
        let rx = udp::PacketBuffer::new(
            Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4])) as &mut [_],
            Box::leak(Box::new([0u8; 1200])) as &mut [_],
        );
        let tx = udp::PacketBuffer::new(
            Box::leak(Box::new([udp::PacketMetadata::EMPTY; 4])) as &mut [_],
            Box::leak(Box::new([0u8; 1200])) as &mut [_],
        );
        let mut socket = udp::Socket::new(rx, tx);
        let _ = socket.bind(67);
        DhcpServer {
            socket: sockets.add(socket),
            leases: [None; MAX_AP_CLIENTS],
        }
    }

    /// Answers the requests that came in.
    fn serve(&mut self, sockets: &mut SocketSet<'static>) {
        let socket = sockets.get_mut::<udp::Socket>(self.socket);
        let mut request = [0u8; 576];
        while let Ok((n, _)) = socket.recv_slice(&mut request) {
            let mut reply = [0u8; 300];
            if let Some(len) = self.answer(&request[..n], &mut reply) {
                let to = IpEndpoint::new(Ipv4Address::BROADCAST.into(), 68);
                let _ = socket.send_slice(&reply[..len], to);
            }
        }
    }

    /// The reply to one DHCP message, written into `reply`.
    fn answer(&mut self, req: &[u8], reply: &mut [u8; 300]) -> Option<usize> {
        // A BOOTP request with the DHCP magic cookie and an Ethernet address
        if req.len() < 240
            || req[0] != 1
            || req[1] != 1
            || req[2] != 6
            || req[236..240] != DHCP_MAGIC
        {
            return None;
        }
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&req[28..34]);
        let (mut kind, mut wanted) = (0u8, None);
        let mut i = 240;
        while i + 1 < req.len() && req[i] != 255 {
            if req[i] == 0 {
                i += 1;
                continue;
            }
            let len = req[i + 1] as usize;
            let value = req.get(i + 2..i + 2 + len)?;
            match (req[i], len) {
                (53, 1) => kind = value[0],
                (50, 4) => wanted = Some(Ipv4Address::new(value[0], value[1], value[2], value[3])),
                _ => {}
            }
            i += 2 + len;
        }
        if wanted.is_none() && req[12..16] != [0; 4] {
            wanted = Some(Ipv4Address::new(req[12], req[13], req[14], req[15]));
            // ciaddr: renewing
        }
        let (answer, slot) = match kind {
            DHCP_DISCOVER => (DHCP_OFFER, self.lease_for(mac)?),
            DHCP_REQUEST => match self.lease_for(mac) {
                Some(slot) if wanted.is_none_or(|w| w == lease_ip(slot)) => (DHCP_ACK, slot),
                _ => (DHCP_NAK, 0),
            },
            DHCP_RELEASE => {
                for lease in self.leases.iter_mut() {
                    if *lease == Some(mac) {
                        *lease = None;
                    }
                }
                return None;
            }
            _ => return None,
        };

        reply.fill(0);
        reply[0] = 2; // BOOTREPLY
        reply[1] = 1;
        reply[2] = 6;
        reply[4..8].copy_from_slice(&req[4..8]); // xid
        reply[10..12].copy_from_slice(&req[10..12]); // flags
        if answer != DHCP_NAK {
            reply[16..20].copy_from_slice(&lease_ip(slot).octets()); // yiaddr
            reply[20..24].copy_from_slice(&AP_IP.octets()); // siaddr
        }
        reply[28..44].copy_from_slice(&req[28..44]); // chaddr
        reply[236..240].copy_from_slice(&DHCP_MAGIC);
        let mut n = 240;
        let mut option = |code: u8, value: &[u8], n: &mut usize| {
            reply[*n] = code;
            reply[*n + 1] = value.len() as u8;
            reply[*n + 2..*n + 2 + value.len()].copy_from_slice(value);
            *n += 2 + value.len();
        };
        option(53, &[answer], &mut n);
        option(54, &AP_IP.octets(), &mut n); // server identifier
        if answer != DHCP_NAK {
            option(51, &LEASE_SECONDS.to_be_bytes(), &mut n);
            option(
                1,
                &Ipv4Cidr::new(AP_IP, AP_PREFIX).netmask().octets(),
                &mut n,
            );
            option(3, &AP_IP.octets(), &mut n); // router
        }
        reply[n] = 255;
        Some(300) // BOOTP's minimum size
    }

    /// The slot of the address for `mac`: the one it has, or a free one.
    fn lease_for(&mut self, mac: [u8; 6]) -> Option<usize> {
        if let Some(slot) = self.leases.iter().position(|l| *l == Some(mac)) {
            return Some(slot);
        }
        let slot = self.leases.iter().position(Option::is_none)?;
        self.leases[slot] = Some(mac);
        Some(slot)
    }
}

/// The address of lease slot `i`: 192.168.4.(2 + i).
fn lease_ip(slot: usize) -> Ipv4Address {
    let o = AP_IP.octets();
    Ipv4Address::new(o[0], o[1], o[2], 2 + slot as u8)
}
