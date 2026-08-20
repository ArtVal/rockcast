//! Unicast /24 TCP probe fallback when mDNS is blocked.

use std::{
    net::{Ipv4Addr, SocketAddr, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use super::eureka::eureka_fallback;
use super::lan::LanIface;
use super::{CAST_PORT, DiscoveredDevice};

const TCP_PROBE_TIMEOUT: Duration = Duration::from_millis(180);
const SCAN_WORKERS: usize = 64;

pub(super) fn discover_subnet(
    timeout: Duration,
    lan: &[LanIface],
    tx: mpsc::Sender<DiscoveredDevice>,
) {
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

pub(crate) fn probe_cast_host(ip: Ipv4Addr) -> Option<DiscoveredDevice> {
    let cast_addr = SocketAddr::from((ip, CAST_PORT));
    TcpStream::connect_timeout(&cast_addr, TCP_PROBE_TIMEOUT).ok()?;
    Some(eureka_fallback(ip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};

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
