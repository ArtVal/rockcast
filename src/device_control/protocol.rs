//! Device-control v1 frame bounds, player facts, and wire JSON.
//! Protocol parsing is bounded; command execution remains outside DC-012.

use super::output::ChromecastReceiver;
use super::transport::ControlSocket;
use serde::{Deserialize, Serialize, de::IgnoredAny};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

pub(super) const MAX_FRAME_BYTES: usize = 65_536;
const MAX_PAYLOAD_BYTES: usize = 61_440;
pub(super) const HEARTBEAT: Duration = Duration::from_secs(20);
pub(super) const POLL: Duration = Duration::from_millis(200);
pub(super) const NO_IDENTITY_DELAY: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Serialize)]
struct OutboundEnvelope<T: Serialize> {
    protocol_version: u8,
    message_id: Uuid,
    #[serde(rename = "type")]
    kind: String,
    sent_at: String,
    payload: T,
}

#[derive(Serialize)]
pub(crate) struct HelloPayload {
    supported_protocol_versions: [u8; 1],
}

impl HelloPayload {
    pub(crate) fn v1() -> Self {
        Self {
            supported_protocol_versions: [1],
        }
    }
}

#[derive(Serialize)]
pub(crate) struct RegisterPayload {
    device_type: &'static str,
    app_version: &'static str,
    manifest: DeviceManifest,
}

impl RegisterPayload {
    pub(crate) fn rockcast() -> Self {
        Self {
            device_type: "rockcast",
            app_version: env!("CARGO_PKG_VERSION"),
            manifest: manifest(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct HeartbeatPayload {
    sequence: u64,
}

impl HeartbeatPayload {
    pub(crate) fn new(sequence: u64) -> Self {
        Self { sequence }
    }
}

#[derive(Serialize)]
pub(crate) struct StateFullPayload {
    snapshot: StateSnapshot,
}

impl StateFullPayload {
    pub(crate) fn new(revision: u64, observed_at: String, state: RuntimeState) -> Self {
        Self {
            snapshot: StateSnapshot {
                state_revision: revision,
                observed_at,
                state,
            },
        }
    }
}

#[derive(Serialize)]
struct StateSnapshot {
    state_revision: u64,
    observed_at: String,
    state: RuntimeState,
}

#[derive(Serialize)]
pub(crate) struct RuntimeState {
    playback: PlaybackRuntimeState,
    volume: VolumeRuntimeState,
    output: OutputRuntimeState,
}

#[derive(Serialize)]
struct PlaybackRuntimeState {
    status: &'static str,
    station_id: Option<String>,
}

#[derive(Serialize)]
struct VolumeRuntimeState {
    level: u8,
    muted: bool,
}

#[derive(Serialize)]
struct OutputRuntimeState {
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    receiver_id: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct Surface {
    pub(crate) surface_id: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) views: [&'static str; 0],
}

#[derive(Serialize)]
pub(crate) struct DeviceManifest {
    manifest_revision: u8,
    roles: [&'static str; 2],
    capabilities: CapabilityManifest,
    entities: [(); 0],
    surfaces: [Surface; 1],
}

#[derive(Serialize)]
struct CapabilityManifest {
    revision: u8,
    items: [Capability; 4],
}

#[derive(Serialize)]
#[serde(tag = "name")]
enum Capability {
    #[serde(rename = "media.playback")]
    Playback {
        version: u8,
        actions: [&'static str; 4],
    },
    #[serde(rename = "media.station")]
    Station {
        version: u8,
        sources: [&'static str; 1],
    },
    #[serde(rename = "media.volume")]
    Volume {
        version: u8,
        minimum: u8,
        maximum: u8,
        step: u8,
        mute: bool,
    },
    #[serde(rename = "voice.input")]
    VoiceInput {
        version: u8,
        formats: [&'static str; 1],
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlayerState {
    pub(crate) playback_status: &'static str,
    pub(crate) station_id: Option<String>,
    pub(crate) volume: u8,
    pub(crate) output_mode: &'static str,
    pub(crate) receiver_id: Option<String>,
}

impl PlayerState {
    pub(crate) fn idle(volume: u8) -> Self {
        Self {
            playback_status: "idle",
            station_id: None,
            volume,
            output_mode: "local",
            receiver_id: None,
        }
    }

    pub(super) fn runtime_state(&self) -> RuntimeState {
        RuntimeState {
            playback: PlaybackRuntimeState {
                status: self.playback_status,
                station_id: self.station_id.clone(),
            },
            // RockCast has no mute operation today; false is a factual local state,
            // not an advertised remote mute command.
            volume: VolumeRuntimeState {
                level: self.volume,
                muted: false,
            },
            output: OutputRuntimeState {
                mode: self.output_mode,
                receiver_id: self.receiver_id.clone(),
            },
        }
    }
}

#[derive(Clone)]
pub(super) struct PublishedState {
    pub(super) revision: u64,
    pub(super) observed_at: String,
    pub(super) state: PlayerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ControlError {
    Unavailable,
    Authentication,
    Protocol,
}

pub(super) enum Inbound {
    Text(String),
    Ping(Vec<u8>),
    Close,
}

#[derive(Deserialize)]
struct InboundTypeEnvelope {
    protocol_version: u8,
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: BTreeMap<String, IgnoredAny>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    payload: ErrorPayload,
}

#[derive(Deserialize)]
struct ErrorPayload {
    error: ErrorCode,
}

#[derive(Deserialize)]
struct ErrorCode {
    code: String,
}

#[derive(Deserialize)]
struct RegisteredEnvelope {
    payload: RegisteredPayload,
}

#[derive(Deserialize)]
struct RegisteredPayload {
    authenticated_device_id: Option<Uuid>,
}

/// The only controller intents RockCast can map to its existing local player.
/// Parsing them here keeps network input away from the UI and playback layers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlayerCommand {
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    PlayStation {
        station_id: String,
    },
    PlayStream {
        station_id: String,
        stream_uri: String,
    },
    SetVolume {
        level: u8,
    },
    ChangeVolume {
        delta: i8,
    },
    SetMute {
        muted: bool,
    },
    ChromecastDiscover,
    ChromecastConnect {
        receiver_id: String,
    },
    ChromecastDisconnect,
    RelayStart,
    RelayStop,
    RelaySetMode {
        mode: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceCommand {
    pub(crate) id: String,
    pub(crate) command: PlayerCommand,
}

#[derive(Deserialize)]
struct CommandIdProbe {
    payload: CommandIdPayloadProbe,
}

#[derive(Deserialize)]
struct CommandIdPayloadProbe {
    command_id: Option<Uuid>,
}

#[derive(Deserialize)]
struct CommandNameProbe {
    payload: CommandNamePayloadProbe,
}

#[derive(Deserialize)]
struct CommandNamePayloadProbe {
    body: CommandNameBodyProbe,
}

#[derive(Deserialize)]
struct CommandNameBodyProbe {
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandEnvelope {
    protocol_version: u8,
    message_id: Uuid,
    #[serde(rename = "type")]
    kind: String,
    sent_at: String,
    payload: CommandPayload,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandPayload {
    command_id: Uuid,
    target: CommandTarget,
    deadline_at: Option<String>,
    body: CommandBody,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandTarget {
    device_id: Uuid,
}

#[derive(Deserialize)]
#[serde(tag = "name", deny_unknown_fields)]
enum CommandBody {
    #[serde(rename = "playback.play")]
    Play,
    #[serde(rename = "playback.pause")]
    Pause,
    #[serde(rename = "playback.stop")]
    Stop,
    #[serde(rename = "playback.next")]
    Next,
    #[serde(rename = "playback.previous")]
    Previous,
    #[serde(rename = "station.play_station")]
    PlayStation { station_id: String },
    #[serde(rename = "station.play_stream")]
    PlayStream {
        source: StationSource,
        /// Catalog identity of the resolved station; the server contract requires it so
        /// the target can publish truthful state without recovering the id from the URL.
        station_id: Option<String>,
        stream_uri: String,
    },
    #[serde(rename = "volume.set_volume")]
    SetVolume { level: u8 },
    #[serde(rename = "volume.change_volume")]
    ChangeVolume { delta: i8 },
    #[serde(rename = "volume.set_mute")]
    SetMute { muted: bool },
    #[serde(rename = "chromecast.discover")]
    ChromecastDiscover,
    #[serde(rename = "chromecast.connect")]
    ChromecastConnect { receiver_id: Uuid },
    #[serde(rename = "chromecast.disconnect")]
    ChromecastDisconnect,
    #[serde(rename = "relay.start")]
    RelayStart,
    #[serde(rename = "relay.stop")]
    RelayStop,
    #[serde(rename = "relay.set_mode")]
    RelaySetMode { mode: String },
}

#[derive(Deserialize)]
enum StationSource {
    #[serde(rename = "rockserver_catalog")]
    RockserverCatalog,
    #[serde(rename = "direct_stream")]
    DirectStream,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommandReject {
    pub(super) command_id: Option<String>,
    pub(super) code: &'static str,
}

pub(super) fn send<T: Serialize>(
    socket: &mut dyn ControlSocket,
    kind: &str,
    payload: T,
) -> Result<(), ControlError> {
    let payload_size = serde_json::to_vec(&payload)
        .map_err(|_| ControlError::Protocol)?
        .len();
    if payload_size > MAX_PAYLOAD_BYTES {
        return Err(ControlError::Protocol);
    }
    let frame = OutboundEnvelope {
        protocol_version: 1,
        message_id: Uuid::new_v4(),
        kind: kind.to_owned(),
        sent_at: timestamp(),
        payload,
    };
    let frame = serde_json::to_string(&frame).map_err(|_| ControlError::Protocol)?;
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ControlError::Protocol);
    }
    socket.send_text(frame)
}

pub(super) fn inbound_type(frame: &str) -> Result<Option<String>, ControlError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ControlError::Protocol);
    }
    let envelope: InboundTypeEnvelope =
        serde_json::from_str(frame).map_err(|_| ControlError::Protocol)?;
    if envelope.protocol_version != 1 {
        return Err(ControlError::Protocol);
    }
    let _ = envelope.payload;
    Ok(envelope.kind)
}

/// Strictly accepts only a command for the authenticated device.  The server
/// routes by identity, but the target is still checked locally so it can never
/// retarget this player or become an arbitrary URL/action channel.
pub(super) fn command_from_frame(
    frame: &str,
    authenticated_device_id: Option<&str>,
) -> Result<DeviceCommand, CommandReject> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(CommandReject {
            command_id: None,
            code: "frame_too_large",
        });
    }
    let command_id = serde_json::from_str::<CommandIdProbe>(frame)
        .ok()
        .and_then(|probe| probe.payload.command_id)
        .map(|id| id.to_string());
    let unsupported = serde_json::from_str::<CommandNameProbe>(frame)
        .ok()
        .and_then(|probe| probe.payload.body.name)
        .is_some_and(|name| !known_command_name(&name));
    let envelope: CommandEnvelope = serde_json::from_str(frame).map_err(|_| CommandReject {
        command_id,
        code: if unsupported {
            "unsupported_command"
        } else {
            "invalid_payload"
        },
    })?;
    if envelope.protocol_version != 1
        || envelope.kind != "device.command"
        || OffsetDateTime::parse(&envelope.sent_at, &Rfc3339).is_err()
    {
        return Err(CommandReject {
            command_id: None,
            code: "invalid_message",
        });
    }
    let _ = envelope.message_id;
    let command_id = envelope.payload.command_id.to_string();
    if envelope
        .payload
        .deadline_at
        .as_deref()
        .is_some_and(|deadline| OffsetDateTime::parse(deadline, &Rfc3339).is_err())
        || authenticated_device_id.and_then(|id| Uuid::parse_str(id).ok())
            != Some(envelope.payload.target.device_id)
    {
        return Err(CommandReject {
            command_id: Some(command_id),
            code: "invalid_payload",
        });
    }
    if matches!(
        &envelope.payload.body,
        CommandBody::PlayStream {
            source: StationSource::DirectStream,
            ..
        }
    ) {
        return Err(CommandReject {
            command_id: Some(command_id),
            code: "unsupported_command",
        });
    }
    let command = command_from_body(envelope.payload.body).ok_or_else(|| CommandReject {
        command_id: Some(command_id.clone()),
        code: "invalid_payload",
    })?;
    Ok(DeviceCommand {
        id: command_id,
        command,
    })
}

fn known_command_name(name: &str) -> bool {
    matches!(
        name,
        "playback.play"
            | "playback.pause"
            | "playback.stop"
            | "playback.next"
            | "playback.previous"
            | "station.play_station"
            | "station.play_stream"
            | "volume.set_volume"
            | "volume.change_volume"
            | "volume.set_mute"
            | "chromecast.discover"
            | "chromecast.connect"
            | "chromecast.disconnect"
            | "relay.start"
            | "relay.stop"
            | "relay.set_mode"
    )
}

fn command_from_body(body: CommandBody) -> Option<PlayerCommand> {
    match body {
        CommandBody::Play => Some(PlayerCommand::Play),
        CommandBody::Pause => Some(PlayerCommand::Pause),
        CommandBody::Stop => Some(PlayerCommand::Stop),
        CommandBody::Next => Some(PlayerCommand::Next),
        CommandBody::Previous => Some(PlayerCommand::Previous),
        CommandBody::PlayStation { station_id } => (!station_id.is_empty()
            && station_id.len() <= 128)
            .then_some(PlayerCommand::PlayStation { station_id }),
        CommandBody::PlayStream {
            source: StationSource::RockserverCatalog,
            station_id,
            stream_uri,
        } => {
            // The server contract always carries the resolved catalog id on this
            // variant; without it the player could not publish truthful state.
            let station_id = station_id.filter(|id| !id.is_empty() && id.len() <= 128)?;
            (!stream_uri.is_empty() && stream_uri.len() <= 2_048).then_some(
                PlayerCommand::PlayStream {
                    station_id,
                    stream_uri,
                },
            )
        }
        CommandBody::PlayStream {
            source: StationSource::DirectStream,
            ..
        } => None,
        CommandBody::SetVolume { level } => {
            (level <= 100).then_some(PlayerCommand::SetVolume { level })
        }
        CommandBody::ChangeVolume { delta } => (delta != 0 && (-100..=100).contains(&delta))
            .then_some(PlayerCommand::ChangeVolume { delta }),
        CommandBody::SetMute { muted } => Some(PlayerCommand::SetMute { muted }),
        CommandBody::ChromecastDiscover => Some(PlayerCommand::ChromecastDiscover),
        CommandBody::ChromecastConnect { receiver_id } => Some(PlayerCommand::ChromecastConnect {
            receiver_id: receiver_id.to_string(),
        }),
        CommandBody::ChromecastDisconnect => Some(PlayerCommand::ChromecastDisconnect),
        CommandBody::RelayStart => Some(PlayerCommand::RelayStart),
        CommandBody::RelayStop => Some(PlayerCommand::RelayStop),
        CommandBody::RelaySetMode { mode } => {
            valid_mode(&mode).then_some(PlayerCommand::RelaySetMode { mode })
        }
    }
}

fn valid_mode(mode: &str) -> bool {
    !mode.is_empty()
        && mode.len() <= 32
        && mode
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && mode
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

pub(super) fn command_accepted(
    socket: &mut dyn ControlSocket,
    command_id: &str,
) -> Result<(), ControlError> {
    send(
        socket,
        "command.accepted",
        CommandAcceptedPayload {
            command_id: command_id.to_owned(),
            accepted_at: timestamp(),
        },
    )
}

pub(super) fn command_result(
    socket: &mut dyn ControlSocket,
    command_id: &str,
    result: &CommandResult,
) -> Result<(), ControlError> {
    send(
        socket,
        "command.result",
        CommandResultPayload {
            command_id: command_id.to_owned(),
            status: if result.error.is_some() {
                CommandStatus::Failed
            } else {
                CommandStatus::Succeeded
            },
            completed_at: timestamp(),
            error: result.error.as_ref().map(|error| CommandError {
                code: error.code,
                message: error.message,
                request_id: command_id.to_owned(),
                details: EmptyDetails {},
            }),
            output: result.output.clone(),
        },
    )
}

#[derive(Serialize)]
struct CommandAcceptedPayload {
    command_id: String,
    accepted_at: String,
}

#[derive(Serialize)]
struct CommandResultPayload {
    command_id: String,
    status: CommandStatus,
    completed_at: String,
    error: Option<CommandError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<CommandOutput>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum CommandStatus {
    Succeeded,
    Failed,
}

#[derive(Serialize)]
struct CommandError {
    code: &'static str,
    message: &'static str,
    request_id: String,
    details: EmptyDetails,
}

#[derive(Serialize)]
struct EmptyDetails {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandResult {
    pub(crate) error: Option<CommandFailure>,
    pub(crate) output: Option<CommandOutput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CommandOutput {
    receivers: Vec<ChromecastReceiver>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandFailure {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

impl CommandResult {
    pub(crate) fn succeeded() -> Self {
        Self {
            error: None,
            output: None,
        }
    }
    pub(crate) fn succeeded_with_receivers(receivers: Vec<ChromecastReceiver>) -> Self {
        Self {
            error: None,
            output: Some(CommandOutput { receivers }),
        }
    }
    pub(crate) fn failed(code: &'static str, message: &'static str) -> Self {
        Self {
            error: Some(CommandFailure { code, message }),
            output: None,
        }
    }
}

pub(super) fn is_auth_error(frame: &str) -> bool {
    matches!(
        protocol_error_code(frame).as_deref(),
        Some("authentication_required" | "forbidden")
    )
}

pub(super) fn protocol_error_code(frame: &str) -> Option<String> {
    serde_json::from_str::<ErrorEnvelope>(frame)
        .ok()
        .map(|envelope| envelope.payload.error.code)
}

pub(super) fn registered_device_id(frame: &str) -> Option<String> {
    serde_json::from_str::<RegisteredEnvelope>(frame)
        .ok()
        .and_then(|envelope| envelope.payload.authenticated_device_id)
        .map(|id| id.to_string())
}

pub(super) fn control_endpoint(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let ws = base
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    format!("{ws}/api/v1/devices/connect")
}

pub(super) fn manifest() -> DeviceManifest {
    DeviceManifest {
        manifest_revision: 4,
        roles: ["player", "voice_endpoint"],
        capabilities: CapabilityManifest {
            revision: 3,
            items: [
                Capability::Playback {
                    version: 1,
                    actions: ["play", "stop", "next", "previous"],
                },
                Capability::Station {
                    version: 1,
                    sources: ["rockserver_catalog"],
                },
                Capability::Volume {
                    version: 1,
                    minimum: 0,
                    maximum: 100,
                    step: 1,
                    mute: false,
                },
                Capability::VoiceInput {
                    version: 1,
                    formats: ["pcm16_mono_16000"],
                },
            ],
        },
        entities: [],
        surfaces: [Surface {
            surface_id: "voice.main",
            kind: "voice",
            display_name: "RockCast voice",
            views: [],
        }],
    }
}

pub(super) fn backoff(attempt: u32) -> Duration {
    let seconds = 1_u64 << attempt.min(5);
    // Deterministic bounded jitter avoids synchronized reconnects without
    // introducing timing-flaky tests.
    Duration::from_millis(
        ((seconds * 1_000).min(MAX_BACKOFF.as_millis() as u64) + u64::from(attempt % 4) * 137)
            .min(MAX_BACKOFF.as_millis() as u64),
    )
}

pub(super) fn wait_or_stop(stopped: &AtomicBool, duration: Duration) {
    let until = Instant::now() + duration;
    while !stopped.load(Ordering::Acquire) && Instant::now() < until {
        thread::sleep(Duration::from_millis(50));
    }
}

pub(super) fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}
