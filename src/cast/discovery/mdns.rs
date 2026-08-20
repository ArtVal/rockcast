//! mDNS discovery for `_googlecast._tcp`.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use mdns_sd::{ServiceDaemon, ServiceEvent};

use super::lan::{LanIface, bind_lan_interfaces};
use super::{CAST_PORT, DiscoveredDevice, DiscoveryError};

pub(super) fn discover_mdns(
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
