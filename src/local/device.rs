//! cpal device enumeration for the output picker.

use cpal::traits::{DeviceTrait, HostTrait};

#[derive(Debug, Clone)]
pub struct LocalDeviceInfo {
    pub id: String,
    pub name: String,
    /// cpal device name; `None` — system default.
    pub cpal_name: Option<String>,
}

impl LocalDeviceInfo {
    pub fn label(&self, lang: crate::i18n::Lang) -> String {
        format!("{}  [{}]", self.name, lang.t().this_pc)
    }
}

pub fn list_local_devices(lang: crate::i18n::Lang) -> Vec<LocalDeviceInfo> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().and_then(|d| d.name().ok());
    let speakers = lang.t().pc_speakers;

    let mut out = Vec::new();
    let Ok(devices) = host.output_devices() else {
        out.push(LocalDeviceInfo {
            id: "local:default".into(),
            name: speakers.into(),
            cpal_name: None,
        });
        return out;
    };

    for device in devices {
        let Ok(name) = device.name() else {
            continue;
        };
        let is_default = default_name.as_ref() == Some(&name);
        let display = if is_default {
            format!("{name} ★")
        } else {
            name.clone()
        };
        out.push(LocalDeviceInfo {
            id: format!("local:{name}"),
            name: display,
            cpal_name: Some(name),
        });
    }

    if out.is_empty() {
        out.push(LocalDeviceInfo {
            id: "local:default".into(),
            name: speakers.into(),
            cpal_name: None,
        });
    } else {
        // Default first.
        out.sort_by(|a, b| {
            let ad = a.name.contains('★');
            let bd = b.name.contains('★');
            match (ad, bd) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });
    }
    out
}
