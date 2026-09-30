//! Background → UI channel messages.

use crate::{cast::CastDeviceInfo, output::OutputDevice, stations::Station};

pub(crate) enum UiMsg {
    Stations {
        list: Vec<Station>,
        source: String,
        request_id: u64,
        /// false = local catalog (enrich still running), true = final.
        finished: bool,
        total: Option<usize>,
        has_more: bool,
    },
    MoreStationsLoaded {
        list: Vec<Station>,
        request_id: u64,
        offset: usize,
        total: Option<usize>,
        has_more: bool,
        next_cursor: Option<String>,
    },
    MoreStationsFailed {
        request_id: u64,
        error: String,
    },
    DeviceFound(OutputDevice),
    DevicesFinished(String),
    RemoteChromecastDiscovery {
        command_id: String,
        result: Result<Vec<CastDeviceInfo>, ()>,
    },
    StationIcon {
        request_key: String,
        image: Option<crate::station_icons::StationIconImage>,
    },
    VoiceResult(Result<crate::voice::VoiceOutcome, crate::voice::VoiceError>),
    PairingResult {
        request_id: String,
        result: Result<crate::session::AccountProfile, crate::session::PairingPoll>,
    },
    PairingStarted {
        name: String,
        result: Result<crate::session::PairingRequest, crate::session::SessionError>,
    },
    AccountLoaded(
        Result<
            Option<(crate::session::AccountProfile, Vec<crate::session::Device>)>,
            crate::session::SessionError,
        >,
    ),
    StationResolved {
        station_id: String,
        result: Result<Station, String>,
        auto_play: bool,
    },
    PersonalSyncResult {
        generation: u64,
        result: Result<crate::personal_sync::SyncOutcome, crate::personal_sync::SyncFailure>,
    },
}

pub(super) fn same_output_device(left: &OutputDevice, right: &OutputDevice) -> bool {
    match (left, right) {
        (OutputDevice::Local(a), OutputDevice::Local(b)) => a.id == b.id,
        (OutputDevice::Cast(a), OutputDevice::Cast(b)) => a.discovered.host == b.discovered.host,
        _ => false,
    }
}
