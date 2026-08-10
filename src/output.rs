//! Output devices: Google Cast and local PC speakers.

use crate::cast::{CastDeviceInfo, CastService};
use crate::i18n::{self, Lang};
use crate::local::{LocalDeviceInfo, list_local_devices};
use std::time::Duration;

#[derive(Clone)]
pub enum OutputDevice {
    Cast(CastDeviceInfo),
    Local(LocalDeviceInfo),
}

impl OutputDevice {
    pub fn id(&self) -> &str {
        match self {
            Self::Cast(d) => d.discovered.id.as_str(),
            Self::Local(d) => d.id.as_str(),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Cast(d) => d.discovered.name.as_str(),
            Self::Local(d) => d.name.as_str(),
        }
    }

    pub fn label(&self, lang: Lang) -> String {
        match self {
            Self::Cast(d) => d.label(),
            Self::Local(d) => d.label(lang),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    pub fn as_cast(&self) -> Option<&CastDeviceInfo> {
        match self {
            Self::Cast(d) => Some(d),
            Self::Local(_) => None,
        }
    }
}

/// Local speakers + Cast on the network. Local devices always come first.
pub fn scan_all(cast_timeout: Duration, lang: Lang) -> (Vec<OutputDevice>, String) {
    let t = lang.t();
    let mut devices: Vec<OutputDevice> = list_local_devices(lang)
        .into_iter()
        .map(OutputDevice::Local)
        .collect();
    let local_n = devices.len();

    match CastService::scan(cast_timeout) {
        Ok(cast) => {
            let cast_n = cast.len();
            devices.extend(cast.into_iter().map(OutputDevice::Cast));
            let status = if cast_n == 0 {
                i18n::fmt1(t.cast_none, local_n)
            } else {
                let pick = devices
                    .iter()
                    .position(|d| match d {
                        OutputDevice::Cast(c) => {
                            let b = format!(
                                "{} {}",
                                c.discovered.name, c.discovered.model
                            )
                            .to_lowercase();
                            b.contains("jbl") || b.contains("bar")
                        }
                        OutputDevice::Local(_) => false,
                    })
                    .or_else(|| devices.iter().position(|d| d.is_local()))
                    .unwrap_or(0);
                i18n::fmt3(
                    t.cast_found,
                    local_n,
                    cast_n,
                    devices[pick].label(lang),
                )
            };
            (devices, status)
        }
        Err(e) => {
            let status = i18n::fmt2(t.cast_err, local_n, e);
            (devices, status)
        }
    }
}
