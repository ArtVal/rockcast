//! RockServer device-control v1 lifecycle for the local RockCast player.
//! It publishes local facts only; remote command execution remains outside DC-012.

#[path = "device_control/output.rs"]
mod output;
#[path = "device_control/protocol.rs"]
mod protocol;
#[cfg(test)]
#[path = "device_control/tests.rs"]
mod tests;
#[path = "device_control/transport.rs"]
mod transport;

use crate::{
    playback::PlaybackPhase,
    rockserver::RuntimeConfig,
    session::{AccountClient, OsCredentialStore, SessionError},
};
pub(crate) use output::{ChromecastDiscovery, LocalChromecastDiscovery, ReceiverCache};
use parking_lot::Mutex;
pub(crate) use protocol::{
    CommandResult, DeviceCommand, PlayerCommand, PlayerState, StationPresentation,
};
use protocol::{
    ControlError, HEARTBEAT, HeartbeatPayload, HelloPayload, Inbound, NO_IDENTITY_DELAY, POLL,
    PublishedState, RegisterPayload, StateFullPayload, backoff, command_accepted,
    command_from_frame, command_result, control_endpoint, inbound_type, is_auth_error,
    protocol_error_code, registered_device_id, send, timestamp, wait_or_stop,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use transport::{ControlSocket, DeviceControlTransport, TungsteniteTransport};

trait DeviceControlAuth: Send + Sync {
    fn access_token(&self, force_renewal: bool) -> Result<Option<String>, SessionError>;
}

struct AccountAuth {
    config: RuntimeConfig,
}

impl DeviceControlAuth for AccountAuth {
    fn access_token(&self, force_renewal: bool) -> Result<Option<String>, SessionError> {
        AccountClient::new(self.config.clone(), OsCredentialStore)
            .device_control_access_token(force_renewal)
    }
}

struct ClientInner {
    config: RuntimeConfig,
    auth: Arc<dyn DeviceControlAuth>,
    transport: Arc<dyn DeviceControlTransport>,
    state: Mutex<Option<PublishedState>>,
    authenticated_device_id: Mutex<Option<String>>,
    commands: Mutex<CommandBook>,
    wake_ui: Arc<dyn Fn() + Send + Sync>,
    stopped: AtomicBool,
    running: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}

const MAX_DEDUPED_COMMANDS: usize = 64;
const MAX_PENDING_COMMANDS: usize = 64;

struct CommandBook {
    queued: VecDeque<DeviceCommand>,
    in_flight: HashMap<String, DeviceCommand>,
    completed: HashMap<String, (CommandResult, bool)>,
    completed_order: VecDeque<String>,
}

impl CommandBook {
    fn new() -> Self {
        Self {
            queued: VecDeque::new(),
            in_flight: HashMap::new(),
            completed: HashMap::new(),
            completed_order: VecDeque::new(),
        }
    }

    fn complete(&mut self, command_id: &str, result: CommandResult) {
        if self.in_flight.remove(command_id).is_none() {
            return;
        }
        self.completed.insert(command_id.into(), (result, false));
        self.completed_order.push_back(command_id.into());
        while self.completed_order.len() > MAX_DEDUPED_COMMANDS {
            if let Some(oldest) = self.completed_order.pop_front() {
                self.completed.remove(&oldest);
            }
        }
    }
}

/// One bounded, process-local connection loop. It neither creates nor stores
/// identity: pairing credentials remain exclusively in `session.dpapi`.
pub(crate) struct DeviceControlClient {
    inner: Arc<ClientInner>,
}

impl DeviceControlClient {
    pub(crate) fn new(
        config: RuntimeConfig,
        initial_revision: u64,
        wake_ui: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let auth = Arc::new(AccountAuth {
            config: config.clone(),
        });
        Self::with_parts_and_wake(
            config,
            initial_revision,
            auth,
            Arc::new(TungsteniteTransport),
            wake_ui,
        )
    }

    #[cfg(test)]
    fn with_parts(
        config: RuntimeConfig,
        initial_revision: u64,
        auth: Arc<dyn DeviceControlAuth>,
        transport: Arc<dyn DeviceControlTransport>,
    ) -> Self {
        Self::with_parts_and_wake(config, initial_revision, auth, transport, Arc::new(|| {}))
    }

    fn with_parts_and_wake(
        config: RuntimeConfig,
        initial_revision: u64,
        auth: Arc<dyn DeviceControlAuth>,
        transport: Arc<dyn DeviceControlTransport>,
        wake_ui: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let client = Self {
            inner: Arc::new(ClientInner {
                config,
                auth,
                transport,
                state: Mutex::new(None),
                authenticated_device_id: Mutex::new(None),
                commands: Mutex::new(CommandBook::new()),
                wake_ui,
                stopped: AtomicBool::new(false),
                running: AtomicBool::new(false),
                worker: Mutex::new(None),
            }),
        };
        // The first UI snapshot becomes initial_revision + 1. Keeping the
        // counter in AppSettings prevents a restart from replaying stale state.
        client.inner.state.lock().replace(PublishedState {
            revision: initial_revision,
            observed_at: String::new(),
            state: PlayerState::idle(0),
        });
        client.start();
        client
    }

    fn start(&self) {
        if self.inner.running.swap(true, Ordering::AcqRel) {
            return;
        }
        let inner = Arc::clone(&self.inner);
        let worker = thread::Builder::new()
            .name("rockcast-device-control".into())
            .spawn(move || run_worker(inner));
        match worker {
            Ok(worker) => *self.inner.worker.lock() = Some(worker),
            Err(_) => self.inner.running.store(false, Ordering::Release),
        }
    }

    /// Replaces a single latest-value slot; there is no unbounded state queue.
    /// Returns the durable revision the caller should save with AppSettings.
    pub(crate) fn publish(&self, state: PlayerState) -> Option<u64> {
        advance_published_state(&self.inner.state, state)
    }

    pub(crate) fn shutdown(&self) {
        self.inner.stopped.store(true, Ordering::Release);
        self.inner.worker.lock().take();
    }

    /// The UI thread remains the sole PlaybackController owner.
    pub(crate) fn take_command(&self) -> Option<DeviceCommand> {
        let mut commands = self.inner.commands.lock();
        let command = commands.queued.pop_front()?;
        commands
            .in_flight
            .insert(command.id.clone(), command.clone());
        Some(command)
    }

    pub(crate) fn complete_command(&self, command_id: &str, result: CommandResult) {
        self.inner.commands.lock().complete(command_id, result);
    }

    pub(crate) fn has_queued_commands(&self) -> bool {
        !self.inner.commands.lock().queued.is_empty()
    }
}

impl Drop for DeviceControlClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_worker(inner: Arc<ClientInner>) {
    let endpoint = control_endpoint(inner.config.base_url());
    let mut forced_renewal = false;
    let mut retry = 0_u32;
    while !inner.stopped.load(Ordering::Acquire) {
        let token = match inner.auth.access_token(forced_renewal) {
            Ok(Some(token)) => token,
            Ok(None) => {
                log::warn!("device control: no paired native credentials; remaining offline");
                forced_renewal = false;
                wait_or_stop(&inner.stopped, NO_IDENTITY_DELAY);
                continue;
            }
            Err(SessionError::Unauthorized) | Err(SessionError::Rejected) => {
                log::warn!(
                    "device control: native session is no longer authorized; remaining offline"
                );
                forced_renewal = false;
                wait_or_stop(&inner.stopped, NO_IDENTITY_DELAY);
                continue;
            }
            Err(error) => {
                log::warn!("device control: cannot obtain native access token: {error}");
                wait_or_stop(&inner.stopped, backoff(retry));
                retry = retry.saturating_add(1);
                continue;
            }
        };
        forced_renewal = false;
        match inner.transport.connect(&endpoint, &token) {
            Ok(socket) => match run_connection(&inner, socket) {
                Ok(()) => retry = 0,
                Err(ControlError::Authentication) => {
                    log::warn!("device control: WebSocket authentication rejected; renewing once");
                    // A server-side auth rejection gets one renewal through the
                    // existing device-session endpoint; that endpoint owns revoke.
                    forced_renewal = true;
                    retry = 0;
                }
                Err(error) => {
                    log::warn!("device control: connected session ended: {error:?}");
                    wait_or_stop(&inner.stopped, backoff(retry));
                    retry = retry.saturating_add(1);
                }
            },
            Err(ControlError::Authentication) => {
                log::warn!("device control: WebSocket rejected credentials; renewing once");
                forced_renewal = true;
            }
            Err(error) => {
                log::warn!("device control: WebSocket connection failed: {error:?}");
                wait_or_stop(&inner.stopped, backoff(retry));
                retry = retry.saturating_add(1);
            }
        }
    }
    inner.running.store(false, Ordering::Release);
}

fn run_connection(
    inner: &ClientInner,
    mut socket: Box<dyn ControlSocket>,
) -> Result<(), ControlError> {
    *inner.authenticated_device_id.lock() = None;
    send(&mut *socket, "protocol.hello", HelloPayload::v1())?;
    let registration_deadline = Instant::now() + Duration::from_secs(10);
    wait_for(
        &mut *socket,
        registration_deadline,
        "protocol.welcome",
        inner,
    )?;
    send(&mut *socket, "device.register", RegisterPayload::rockcast())?;
    wait_for(
        &mut *socket,
        registration_deadline,
        "device.registered",
        inner,
    )?;
    log::info!("device control: registered; publishing authoritative state");

    // A full snapshot is sent for every fresh connection, even if no local
    // state changed while the socket was down.
    let mut sent_revision = send_full(&mut *socket, inner, 0)?;
    let mut heartbeat_sequence = 0_u64;
    let mut next_heartbeat = Instant::now() + HEARTBEAT;
    loop {
        if inner.stopped.load(Ordering::Acquire) {
            socket.close();
            return Ok(());
        }
        if state_is_newer_than(&inner.state, sent_revision) {
            sent_revision = send_full(&mut *socket, inner, sent_revision)?;
        }
        if Instant::now() >= next_heartbeat {
            send(
                &mut *socket,
                "device.heartbeat",
                HeartbeatPayload::new(heartbeat_sequence),
            )?;
            heartbeat_sequence = heartbeat_sequence.saturating_add(1);
            next_heartbeat = Instant::now() + HEARTBEAT;
        }
        send_pending_results(&mut *socket, inner)?;
        match socket.read()? {
            None => thread::sleep(POLL),
            Some(Inbound::Ping(payload)) => {
                // Tungstenite normally responds during read; an explicit pong is
                // not required for the RockServer application heartbeat.
                let _ = payload;
            }
            Some(Inbound::Close) => return Err(ControlError::Unavailable),
            Some(Inbound::Text(text)) => match inbound_type(&text)? {
                Some(kind) if kind == "device.resync_requested" => {
                    sent_revision = send_full(&mut *socket, inner, sent_revision)?;
                }
                Some(kind) if kind == "protocol.error" && is_auth_error(&text) => {
                    return Err(ControlError::Authentication);
                }
                Some(kind) if kind == "protocol.error" => {
                    log::warn!(
                        "device control: server protocol error: {}",
                        protocol_error_code(&text).unwrap_or_else(|| "unknown".into())
                    );
                    return Err(ControlError::Protocol);
                }
                Some(kind) if kind == "device.command" => {
                    receive_command(&mut *socket, inner, &text)?
                }
                Some(_) | None => {}
            },
        }
    }
}

fn state_is_newer_than(state: &Mutex<Option<PublishedState>>, sent_revision: u64) -> bool {
    let revision = { state.lock().as_ref().map(|current| current.revision) };
    revision.is_some_and(|revision| revision > sent_revision)
}

/// Advances the latest-value state slot by one truthful fact.
///
/// Equal states are deduplicated (no revision bump, `None` returned), while any
/// changed state advances the revision strictly monotonically. The startup
/// placeholder carries an empty `observed_at`, so the first snapshot always
/// advances the revision persisted in `AppSettings` — a restart can never
/// replay a stale revision.
fn advance_published_state(slot: &Mutex<Option<PublishedState>>, next: PlayerState) -> Option<u64> {
    let mut published = slot.lock();
    let current = published.as_mut()?;
    if current.state == next && !current.observed_at.is_empty() {
        return None;
    }
    current.revision = current.revision.saturating_add(1).max(1);
    current.observed_at = timestamp();
    current.state = next;
    Some(current.revision)
}

/// Maps the authoritative playback phase and the station chosen for the
/// current start lifecycle onto the wire playback status
/// (live-control §4.3/§4.5).
///
/// `Opening` means an accepted station is loading (`buffering`), `Stopping`
/// is `stopped`, `Failed` is `error`. A pristine `Idle` (no station chosen in
/// this session) stays `idle`; an `Idle` that follows a chosen station keeps
/// `stopped` so the last station context survives until a new choice.
/// `paused` is never returned: RockCast does not support pause and does not
/// advertise it.
pub(crate) fn playback_status(phase: PlaybackPhase, station_id: Option<&str>) -> &'static str {
    match phase {
        PlaybackPhase::Opening { .. } => "buffering",
        PlaybackPhase::Playing { .. } => "playing",
        PlaybackPhase::Stopping { .. } => "stopped",
        PlaybackPhase::Failed { .. } => "error",
        PlaybackPhase::Idle => {
            if station_id.is_some() {
                "stopped"
            } else {
                "idle"
            }
        }
    }
}

fn receive_command(
    socket: &mut dyn ControlSocket,
    inner: &ClientInner,
    frame: &str,
) -> Result<(), ControlError> {
    let device_id = inner.authenticated_device_id.lock().clone();
    let command = match command_from_frame(frame, device_id.as_deref()) {
        Ok(command) => command,
        Err(reject) => {
            if let Some(command_id) = reject.command_id {
                command_result(
                    socket,
                    &command_id,
                    &CommandResult::failed(reject.code, "Command was rejected"),
                )?;
            }
            return Ok(());
        }
    };
    if !command_is_advertised(&command.command) {
        return command_result(
            socket,
            &command.id,
            &CommandResult::failed(
                "capability_not_supported",
                "Command is not supported by this player",
            ),
        );
    }
    let mut commands = inner.commands.lock();
    if commands.in_flight.contains_key(&command.id)
        || commands.queued.iter().any(|queued| queued.id == command.id)
    {
        return Ok(());
    }
    if let Some((result, delivered)) = commands.completed.get_mut(&command.id) {
        if *delivered {
            return Ok(());
        }
        let result = result.clone();
        drop(commands);
        command_result(socket, &command.id, &result)?;
        if let Some((_, delivered)) = inner.commands.lock().completed.get_mut(&command.id) {
            *delivered = true;
        }
        return Ok(());
    }
    if commands.queued.len() + commands.in_flight.len() >= MAX_PENDING_COMMANDS {
        drop(commands);
        return command_result(
            socket,
            &command.id,
            &CommandResult::failed("command_timeout", "Device command queue is full"),
        );
    }
    commands.queued.push_back(command.clone());
    drop(commands);
    (inner.wake_ui)();
    command_accepted(socket, &command.id)
}

fn command_is_advertised(command: &PlayerCommand) -> bool {
    matches!(
        command,
        PlayerCommand::Play
            | PlayerCommand::Stop
            | PlayerCommand::Next
            | PlayerCommand::Previous
            | PlayerCommand::PlayStation { .. }
            | PlayerCommand::PlayStream { .. }
            | PlayerCommand::SetVolume { .. }
            | PlayerCommand::ChangeVolume { .. }
    )
}

fn send_pending_results(
    socket: &mut dyn ControlSocket,
    inner: &ClientInner,
) -> Result<(), ControlError> {
    let pending = {
        let commands = inner.commands.lock();
        commands
            .completed
            .iter()
            .filter(|(_, (_, delivered))| !*delivered)
            .map(|(id, (result, _))| (id.clone(), result.clone()))
            .collect::<Vec<_>>()
    };
    for (command_id, result) in pending {
        command_result(socket, &command_id, &result)?;
        if let Some((_, delivered)) = inner.commands.lock().completed.get_mut(&command_id) {
            *delivered = true;
        }
    }
    Ok(())
}

fn wait_for(
    socket: &mut dyn ControlSocket,
    deadline: Instant,
    expected: &str,
    inner: &ClientInner,
) -> Result<(), ControlError> {
    while Instant::now() < deadline {
        if inner.stopped.load(Ordering::Acquire) {
            socket.close();
            return Err(ControlError::Unavailable);
        }
        match socket.read()? {
            None => thread::sleep(POLL),
            Some(Inbound::Close) => return Err(ControlError::Unavailable),
            Some(Inbound::Ping(_)) => {}
            Some(Inbound::Text(text)) => match inbound_type(&text)? {
                Some(kind) if kind == expected => {
                    if expected == "device.registered" {
                        *inner.authenticated_device_id.lock() = registered_device_id(&text);
                    }
                    return Ok(());
                }
                Some(kind) if kind == "protocol.error" && is_auth_error(&text) => {
                    return Err(ControlError::Authentication);
                }
                Some(kind) if kind == "protocol.error" => {
                    log::warn!(
                        "device control: server rejected {expected}: {}",
                        protocol_error_code(&text).unwrap_or_else(|| "unknown".into())
                    );
                    return Err(ControlError::Protocol);
                }
                Some(_) | None => {}
            },
        }
    }
    Err(ControlError::Protocol)
}

fn send_full(
    socket: &mut dyn ControlSocket,
    inner: &ClientInner,
    previous: u64,
) -> Result<u64, ControlError> {
    let current = inner.state.lock().clone().ok_or(ControlError::Protocol)?;
    if current.revision == 0 || current.observed_at.is_empty() {
        return Err(ControlError::Protocol);
    }
    let revision = current.revision.max(previous.saturating_add(1));
    send(
        socket,
        "device.state_full",
        StateFullPayload::new(revision, current.observed_at, current.state.runtime_state()),
    )?;
    Ok(revision)
}
