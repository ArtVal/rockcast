use std::net::{Ipv4Addr, SocketAddr};

pub fn advertise_ipv4_near(cast_host: &str) -> Option<Ipv4Addr> {
    if let Ok(ip) = std::env::var("ROCKCAST_RELAY_ADVERTISE_IP")
        && let Ok(ip) = ip.parse::<Ipv4Addr>()
    {
        return Some(ip);
    }
    let peer = resolve_ipv4(cast_host)?;
    let Ok(ifaces) = if_addrs::get_if_addrs() else {
        return None;
    };

    let mut same_net: Option<(i32, Ipv4Addr)> = None;
    let mut best_lan: Option<(i32, Ipv4Addr)> = None;

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
        if !ip.is_private() && !is_apipa(ip) {
            continue;
        }
        let score = score_lan(&iface.name, ip, is_apipa(ip), iface.is_oper_up());
        let netmask = v4.netmask;
        if in_same_subnet(ip, netmask, peer) && same_net.map(|(s, _)| score > s).unwrap_or(true) {
            same_net = Some((score, ip));
        }
        if best_lan.map(|(s, _)| score > s).unwrap_or(true) {
            best_lan = Some((score, ip));
        }
    }

    same_net.or(best_lan).map(|(_, ip)| ip)
}

fn resolve_ipv4(host: &str) -> Option<Ipv4Addr> {
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Some(ip);
    }
    use std::net::ToSocketAddrs;
    let addrs = format!("{host}:0").to_socket_addrs().ok()?;
    for a in addrs {
        if let SocketAddr::V4(v4) = a {
            return Some(*v4.ip());
        }
    }
    None
}

fn in_same_subnet(ip: Ipv4Addr, mask: Ipv4Addr, peer: Ipv4Addr) -> bool {
    let ip_u = u32::from(ip);
    let mask_u = u32::from(mask);
    let peer_u = u32::from(peer);
    (ip_u & mask_u) == (peer_u & mask_u)
}

fn is_apipa(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 169 && o[1] == 254
}

fn score_lan(name: &str, ip: Ipv4Addr, apipa: bool, oper_up: bool) -> i32 {
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
    let o = ip.octets();
    if o[0] == 192 && o[1] == 168 {
        score += 50;
    } else if o[0] == 10 {
        score += 30;
    } else if o[0] == 172 && (16..=31).contains(&o[1]) {
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
