//! Discover Google Cast devices via mDNS, with a unicast subnet scan fallback.
//!
//! Amnezia / WireGuard split-tunnel often breaks multicast even when LAN unicast
//! works. In that case mDNS finds nothing, but Cast still answers on TCP
//! 8008/8009 — so we probe the LAN /24 and read `/setup/eureka_info`.

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent};
use thiserror::Error;

const CAST_PORT: u16 = 8009;
const EUREKA_PORT: u16 = 8008;
const TCP_PROBE_TIMEOUT: Duration = Duration::from_millis(180);
const EUREKA_TIMEOUT: Duration = Duration::from_millis(400);
const SCAN_WORKERS: usize = 64;

#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub name: String,
    pub model: String,
    pub host: String,
    pub port: u16,
    pub id: String,
}

impl DiscoveredDevice {
    pub fn label(&self) -> String {
        let model = if self.model.is_empty() {
            "Chromecast"
        } else {
            self.model.as_str()
        };
        format!("{}  [{model}]", self.name)
    }
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("mDNS: {0}")]
    Mdns(String),
}

pub fn discover(timeout: Duration) -> Result<Vec<DiscoveredDevice>, DiscoveryError> {
    discover_streaming(timeout, |_| {})
}

pub fn discover_streaming(
    timeout: Duration,
    mut on_found: impl FnMut(DiscoveredDevice),
) -> Result<Vec<DiscoveredDevice>, DiscoveryError> {
    let lan = collect_lan_ipv4();
    let (tx, rx) = mpsc::channel::<DiscoveredDevice>();

    // Parallel: mDNS (often broken under VPN) + unicast /24 probe (works with Amnezia).
    let mdns_tx = tx.clone();
    let mdns_lan = lan.clone();
    let mdns_timeout = timeout;
    let mdns_h = thread::spawn(move || {
        if let Err(e) = discover_mdns(mdns_timeout, &mdns_lan, mdns_tx) {
            log::warn!("Cast mDNS: {e}");
        }
    });

    let scan_tx = tx;
    let scan_lan = lan;
    let scan_timeout = timeout;
    let scan_h = thread::spawn(move || {
        discover_subnet(scan_timeout, &scan_lan, scan_tx);
    });

    let mut found: HashMap<String, DiscoveredDevice> = HashMap::new();
    let deadline = Instant::now() + timeout + Duration::from_millis(500);
    while Instant::now() < deadline {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(200));
        match rx.recv_timeout(wait) {
            Ok(dev) => {
                let host = dev.host.clone();
                merge_device(&mut found, dev);
                if let Some(device) = found.get(&host) {
                    on_found(device.clone());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if mdns_h.is_finished() && scan_h.is_finished() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = mdns_h.join();
    let _ = scan_h.join();
    while let Ok(dev) = rx.try_recv() {
        let host = dev.host.clone();
        merge_device(&mut found, dev);
        if let Some(device) = found.get(&host) {
            on_found(device.clone());
        }
    }

    let mut devices: Vec<_> = found.into_values().collect();
    devices.sort_by(|a, b| {
        let aj = is_jbl_like(a);
        let bj = is_jbl_like(b);
        match (aj, bj) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
    log::info!("Cast discovery: {} devices total", devices.len());
    Ok(devices)
}

fn is_jbl_like(d: &DiscoveredDevice) -> bool {
    let blob = format!("{} {}", d.name, d.model).to_lowercase();
    blob.contains("jbl") || blob.contains("bar") || blob.contains("harman")
}

fn merge_device(found: &mut HashMap<String, DiscoveredDevice>, dev: DiscoveredDevice) {
    let key = dev.host.clone();
    match found.get(&key) {
        Some(old) => {
            // Prefer richer TXT/name from either source.
            let mut merged = old.clone();
            if (merged.name.is_empty()
                || (merged.name.starts_with('_') && !dev.name.is_empty())
                || (dev.name.chars().count() > merged.name.chars().count()
                    && !dev.name.contains("._googlecast")))
                && !dev.name.is_empty()
            {
                merged.name = dev.name;
            }
            if merged.model.is_empty() && !dev.model.is_empty() {
                merged.model = dev.model;
            }
            if merged.id.len() < dev.id.len() {
                merged.id = dev.id;
            }
            merged.port = CAST_PORT;
            found.insert(key, merged);
        }
        None => {
            log::info!(
                "Cast found: {} [{}] {}:{} id={}",
                dev.name,
                dev.model,
                dev.host,
                dev.port,
                dev.id
            );
            found.insert(key, dev);
        }
    }
}

fn discover_mdns(
    timeout: Duration,
    lan: &[LanIface],
    tx: mpsc::Sender<DiscoveredDevice>,
) -> Result<(), DiscoveryError> {
    let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError::Mdns(e.to_string()))?;
    bind_lan_interfaces(&daemon, lan);
    let _ = daemon.accept_unsolicited(true);

    let receiver = daemon
        .browse("_googlecast._tcp.local.")
        .map_err(|e| DiscoveryError::Mdns(e.to_string()))?;

    let deadline = Instant::now() + timeout;
    let mut last_new = None::<Instant>;
    let settle = Duration::from_millis(1800);
    let mut count = 0usize;

    while Instant::now() < deadline {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(400));
        match receiver.recv_timeout(wait) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                if let Some(device) = from_resolved(&info) {
                    count += 1;
                    let _ = tx.send(device);
                    last_new = Some(Instant::now());
                }
            }
            Ok(ServiceEvent::ServiceFound(ty, name)) => {
                log::debug!("Cast mDNS found: {name} ({ty})");
            }
            Ok(_) | Err(_) => {}
        }
        if let Some(t) = last_new
            && t.elapsed() >= settle
        {
            break;
        }
    }

    let _ = daemon.shutdown();
    log::info!("Cast mDNS: {count} resolved");
    Ok(())
}

fn discover_subnet(timeout: Duration, lan: &[LanIface], tx: mpsc::Sender<DiscoveredDevice>) {
    let mut targets = Vec::new();
    let mut seen_net = std::collections::HashSet::new();
    for iface in lan {
        let o = iface.ip.octets();
        let net = (o[0], o[1], o[2]);
        if !seen_net.insert(net) {
            continue;
        }
        log::info!(
            "Cast subnet scan: {}.{}.{}.0/24 via \"{}\"",
            o[0],
            o[1],
            o[2],
            iface.name
        );
        for host in 1u8..=254 {
            let ip = Ipv4Addr::new(o[0], o[1], o[2], host);
            if ip == iface.ip {
                continue;
            }
            targets.push(ip);
        }
    }

    if targets.is_empty() {
        log::warn!("Cast subnet scan: no LAN prefixes to probe");
        return;
    }

    let deadline = Instant::now() + timeout;
    let n_workers = SCAN_WORKERS.min(targets.len()).max(1);
    let chunk_size = targets.len().div_ceil(n_workers);
    let mut workers = Vec::new();
    for slice in targets.chunks(chunk_size) {
        let slice = slice.to_vec();
        let tx = tx.clone();
        workers.push(thread::spawn(move || {
            for ip in slice {
                if Instant::now() >= deadline {
                    break;
                }
                if let Some(dev) = probe_cast_host(ip) {
                    let _ = tx.send(dev);
                }
            }
        }));
    }
    for w in workers {
        let _ = w.join();
    }
}

fn probe_cast_host(ip: Ipv4Addr) -> Option<DiscoveredDevice> {
    let cast_addr = SocketAddr::from((ip, CAST_PORT));
    TcpStream::connect_timeout(&cast_addr, TCP_PROBE_TIMEOUT).ok()?;

    let info = fetch_eureka(ip).unwrap_or_else(|| EurekaInfo {
        name: format!("Cast {ip}"),
        model: String::new(),
        id: ip.to_string(),
    });

    Some(DiscoveredDevice {
        name: info.name,
        model: if info.model.is_empty() {
            "Cast".into()
        } else {
            info.model
        },
        host: ip.to_string(),
        port: CAST_PORT,
        id: info.id,
    })
}

struct EurekaInfo {
    name: String,
    model: String,
    id: String,
}

fn fetch_eureka(ip: Ipv4Addr) -> Option<EurekaInfo> {
    let addr = SocketAddr::from((ip, EUREKA_PORT));
    let mut stream = TcpStream::connect_timeout(&addr, EUREKA_TIMEOUT).ok()?;
    let _ = stream.set_read_timeout(Some(EUREKA_TIMEOUT));
    let _ = stream.set_write_timeout(Some(EUREKA_TIMEOUT));

    let req = format!(
        "GET /setup/eureka_info?params=name,device_info,build_info HTTP/1.1\r\n\
         Host: {ip}\r\n\
         Connection: close\r\n\
         Accept: */*\r\n\
         User-Agent: RockCast/0.1\r\n\
         \r\n"
    );
    stream.write_all(req.as_bytes()).ok()?;

    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1)?;
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;

    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();
    let model = v
        .pointer("/device_info/manufacturer")
        .and_then(|x| x.as_str())
        .or_else(|| {
            v.pointer("/device_info/model_name")
                .and_then(|x| x.as_str())
        })
        .unwrap_or("")
        .to_string();
    let id = v
        .get("ssdp_udn")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("mac_address").and_then(|x| x.as_str()))
        .unwrap_or("")
        .to_string();
    let id = if id.is_empty() { ip.to_string() } else { id };
    let name = if name.is_empty() {
        format!("Cast {ip}")
    } else {
        name
    };

    Some(EurekaInfo { name, model, id })
}

#[derive(Debug, Clone)]
struct LanIface {
    name: String,
    ip: Ipv4Addr,
    score: i32,
}

fn bind_lan_interfaces(daemon: &ServiceDaemon, lan: &[LanIface]) {
    let _ = daemon.disable_interface(IfKind::IPv6);
    let _ = daemon.disable_interface(IfKind::LoopbackV4);
    let _ = daemon.disable_interface(IfKind::LoopbackV6);

    if lan.is_empty() {
        log::warn!("Cast mDNS: no LAN IPv4 candidates; falling back to virtual-NIC blacklist");
        disable_virtual_interfaces(daemon);
        return;
    }

    let _ = daemon.disable_interface(IfKind::All);
    for iface in lan.iter().take(3) {
        log::info!(
            "Cast mDNS: using LAN interface \"{}\" ({}) score={}",
            iface.name,
            iface.ip,
            iface.score
        );
        let _ = daemon.enable_interface(IfKind::Addr(IpAddr::V4(iface.ip)));
        let _ = daemon.enable_interface(iface.name.as_str());
    }
}

fn collect_lan_ipv4() -> Vec<LanIface> {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for iface in ifaces {
        if iface.is_loopback() || is_vpn_or_virtual(&iface.name) {
            continue;
        }
        let if_addrs::IfAddr::V4(ref v4) = iface.addr else {
            continue;
        };
        let ip = v4.ip;
        if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
            continue;
        }
        let apipa = is_apipa(ip);
        if !ip.is_private() && !apipa {
            continue;
        }

        let score = score_lan_iface(&iface.name, ip, apipa, iface.is_oper_up());
        out.push(LanIface {
            name: iface.name,
            ip,
            score,
        });
    }

    if out.iter().any(|i| !is_apipa(i.ip)) {
        out.retain(|i| !is_apipa(i.ip));
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

fn score_lan_iface(name: &str, ip: Ipv4Addr, apipa: bool, oper_up: bool) -> i32 {
    let lower = name.to_lowercase();
    let mut score = 0;

    if lower.contains("wi-fi")
        || lower.contains("wifi")
        || lower.contains("wlan")
        || lower.contains("wireless")
        || lower.contains("беспровод")
    {
        score += 100;
    }
    if lower.contains("ethernet") || lower.contains("локальн") {
        score += 80;
    } else if lower.contains("eth") || lower.ends_with(" lan") || lower.contains(" lan ") {
        score += 60;
    }

    let octets = ip.octets();
    if octets[0] == 192 && octets[1] == 168 {
        score += 50;
    } else if octets[0] == 10 {
        score += 30;
    } else if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        score += 10;
    }

    if apipa {
        score -= 40;
    }
    if oper_up {
        score += 20;
    }
    score
}

fn is_apipa(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 169 && o[1] == 254
}

fn is_vpn_or_virtual(name: &str) -> bool {
    let lower = name.to_lowercase();
    const MARKERS: &[&str] = &[
        "amnezia",
        "wintun",
        "wireguard",
        "outline",
        "nordlynx",
        "nordvpn",
        "openvpn",
        "tap-windows",
        "tap-win",
        "tunnel",
        "vpn",
        "utun",
        "warp",
        "cloudflare",
        "zerotier",
        "tailscale",
        "hamachi",
        "radmin",
        "softether",
        "vethernet",
        "hyper-v",
        "virtualbox",
        "vmware",
        "vmnet",
        "wsl",
        "docker",
        "bravetunnel",
        "npcap",
        "loopback",
        "bluetooth",
        "isatap",
        "teredo",
        "microsoft wi-fi direct",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    lower.starts_with("tap")
        || lower.starts_with("tun")
        || lower == "wg"
        || lower.starts_with("wg-")
}

fn disable_virtual_interfaces(daemon: &ServiceDaemon) {
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return;
    };
    let mut seen = std::collections::HashSet::new();
    for iface in ifaces {
        let name = iface.name;
        if !seen.insert(name.clone()) {
            continue;
        }
        if is_vpn_or_virtual(&name) {
            log::info!("Cast mDNS: disabling interface \"{name}\"");
            let _ = daemon.disable_interface(name.as_str());
        }
    }
}

fn from_resolved(info: &mdns_sd::ResolvedService) -> Option<DiscoveredDevice> {
    let props = info.get_properties();
    let name = props
        .get_property_val_str("fn")
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            info.get_fullname()
                .trim_end_matches(".local.")
                .trim_end_matches("._googlecast._tcp")
                .trim_end_matches("_googlecast._tcp")
                .trim_matches('.')
                .to_string()
        });
    let model = props.get_property_val_str("md").unwrap_or("").to_string();
    let id = props
        .get_property_val_str("id")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{name}:{}", info.get_port()));

    let host = pick_host(info)?;
    Some(DiscoveredDevice {
        name,
        model,
        host,
        port: {
            let p = info.get_port();
            if p == 0 { CAST_PORT } else { p }
        },
        id,
    })
}

fn pick_host(info: &mdns_sd::ResolvedService) -> Option<String> {
    let v4 = info.get_addresses_v4();
    let mut link_local = None;
    for addr in &v4 {
        let octets = addr.octets();
        if octets[0] == 169 && octets[1] == 254 {
            link_local.get_or_insert_with(|| addr.to_string());
            continue;
        }
        if octets[0] == 127 {
            continue;
        }
        return Some(addr.to_string());
    }
    if let Some(ll) = link_local {
        return Some(ll);
    }
    info.get_addresses()
        .iter()
        .find(|a| !a.is_loopback())
        .map(|a| a.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpn_names_filtered() {
        assert!(is_vpn_or_virtual("AmneziaVPN"));
        assert!(is_vpn_or_virtual("vEthernet (Default Switch)"));
        assert!(!is_vpn_or_virtual("Беспроводная сеть"));
        assert!(!is_vpn_or_virtual("Wi-Fi"));
    }

    #[test]
    fn probe_known_jbl_on_lan() {
        // Live probe: skip if device offline / not on this LAN.
        let ip = Ipv4Addr::new(192, 168, 31, 109);
        let cast_ok = TcpStream::connect_timeout(
            &SocketAddr::from((ip, CAST_PORT)),
            Duration::from_millis(300),
        )
        .is_ok();
        if !cast_ok {
            eprintln!("skip: {ip}:{CAST_PORT} not open");
            return;
        }
        let dev = probe_cast_host(ip).expect("eureka probe");
        assert_eq!(dev.host, "192.168.31.109");
        assert_eq!(dev.port, CAST_PORT);
        assert!(!dev.name.is_empty());
        eprintln!("probed: {} [{}]", dev.name, dev.model);
    }
}
