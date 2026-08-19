//! Background → UI channel messages.

use crate::{output::OutputDevice, stations::Station};

pub(crate) enum UiMsg {
    Stations {
        list: Vec<Station>,
        source: String,
        /// false = local catalog (enrich still running), true = final.
        finished: bool,
    },
    DeviceFound(OutputDevice),
    DevicesFinished(String),
    VoiceResult(Result<crate::voice::VoiceSearchResult, String>),
}

pub(super) fn same_output_device(left: &OutputDevice, right: &OutputDevice) -> bool {
    match (left, right) {
        (OutputDevice::Local(a), OutputDevice::Local(b)) => a.id == b.id,
        (OutputDevice::Cast(a), OutputDevice::Cast(b)) => a.discovered.host == b.discovered.host,
        _ => false,
    }
}
