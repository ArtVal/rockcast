//! Discover Google Cast devices via mDNS, with a unicast subnet scan fallback.
//!
//! Amnezia / WireGuard split-tunnel often breaks multicast even when LAN unicast
//! works. In that case mDNS finds nothing, but Cast still answers on TCP
//! 8008/8009 — so we probe the LAN /24 and read `/setup/eureka_info`.

mod eureka;
mod lan;
mod mdns;
mod subnet;

use std::{
    collections::HashMap,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

pub(crate) const CAST_PORT: u16 = 8009;

use lan::collect_lan_ipv4;
use mdns::discover_mdns;
use subnet::discover_subnet;

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
