//! LAN interface selection for mDNS and subnet scans.

use std::net::{IpAddr, Ipv4Addr};

use mdns_sd::{IfKind, ServiceDaemon};

#[derive(Debug, Clone)]
pub(super) struct LanIface {
    pub name: String,
    pub ip: Ipv4Addr,
    pub score: i32,
}

pub(super) fn bind_lan_interfaces(daemon: &ServiceDaemon, lan: &[LanIface]) {
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

pub(super) fn collect_lan_ipv4() -> Vec<LanIface> {
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

pub(super) fn is_vpn_or_virtual(name: &str) -> bool {
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
        || lower.starts_with("wg")
        || lower.starts_with("awg")
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

#[cfg(test)]
mod tests {
    use super::is_vpn_or_virtual;

    #[test]
    fn vpn_names_filtered() {
        assert!(is_vpn_or_virtual("AmneziaVPN"));
        assert!(is_vpn_or_virtual("vEthernet (Default Switch)"));
        assert!(is_vpn_or_virtual("tun0"));
        assert!(is_vpn_or_virtual("wg0"));
        assert!(is_vpn_or_virtual("amneziawg0"));
        assert!(!is_vpn_or_virtual("Беспроводная сеть"));
        assert!(!is_vpn_or_virtual("Wi-Fi"));
        assert!(!is_vpn_or_virtual("wlp2s0"));
    }
}
