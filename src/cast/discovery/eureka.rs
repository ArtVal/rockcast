//! HTTP eureka_info probe on port 8008.

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    time::Duration,
};

pub(super) const EUREKA_PORT: u16 = 8008;
const EUREKA_TIMEOUT: Duration = Duration::from_millis(400);

pub(in crate::cast::discovery) struct EurekaInfo {
    name: String,
    model: String,
    id: String,
}

pub(super) fn fetch_eureka(ip: Ipv4Addr) -> Option<EurekaInfo> {
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

pub(super) fn eureka_fallback(ip: Ipv4Addr) -> super::DiscoveredDevice {
    let info = fetch_eureka(ip).unwrap_or_else(|| EurekaInfo {
        name: format!("Cast {ip}"),
        model: String::new(),
        id: ip.to_string(),
    });
    super::DiscoveredDevice {
        name: info.name,
        model: if info.model.is_empty() {
            "Cast".into()
        } else {
            info.model
        },
        host: ip.to_string(),
        port: super::CAST_PORT,
        id: info.id,
    }
}
