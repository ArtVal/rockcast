//! Play, stop, shutdown, and observer wiring.

use crate::{
    device_control::{ChromecastDiscovery, CommandResult, LocalChromecastDiscovery, PlayerCommand},
    output::OutputDevice,
    voice::VoiceControl,
};
use std::time::Duration;
use time::OffsetDateTime;

use super::super::RockCastApp;

const MAX_REMOTE_COMMANDS_PER_FRAME: usize = 16;

#[derive(Debug, PartialEq, Eq)]
enum RemoteCommandPlan {
    PlaySelected,
    Stop,
    PlayRelative(isize),
    PlayStation(usize),
    /// Server-resolved `station.play_stream`: the catalog id and stream URI come from
    /// the command and playback must not require local-catalog membership.
    PlayDelivered {
        station_id: String,
        stream_uri: String,
    },
    SetVolume(u8),
    ChromecastDiscover,
    ChromecastConnect {
        receiver_id: String,
    },
    ChromecastDisconnect,
    RelayStart,
    RelayStop,
}

/// Minimal station for a server-delivered `station.play_stream`. Identity comes from
/// the command's catalog id, never from the stream URL (live-control §4.3); a local
/// catalog entry with the same id would have supplied richer metadata earlier.
fn delivered_station(station_id: &str, stream_uri: &str) -> crate::stations::Station {
    crate::stations::Station::from_primary(
        station_id.to_owned(),
        station_id.to_owned(),
        stream_uri.to_owned(),
        String::new(),
        String::new(),
        0,
        String::new(),
    )
}

fn plan_remote_command(
    command: &PlayerCommand,
    stations: &[crate::stations::Station],
    selected_station: Option<usize>,
    volume: u8,
) -> Result<RemoteCommandPlan, CommandResult> {
    let unavailable = || {
        CommandResult::failed(
            "invalid_payload",
            "Command cannot be applied to local playback state",
        )
    };
    match command {
        PlayerCommand::Play => Ok(RemoteCommandPlan::PlaySelected),
        PlayerCommand::Stop => Ok(RemoteCommandPlan::Stop),
        PlayerCommand::Next => selected_station
            .and_then(|current| current.checked_add(1))
            .filter(|next| *next < stations.len())
            .map(|_| RemoteCommandPlan::PlayRelative(1))
            .ok_or_else(unavailable),
        PlayerCommand::Previous => selected_station
            .and_then(|current| current.checked_sub(1))
            .map(|_| RemoteCommandPlan::PlayRelative(-1))
            .ok_or_else(unavailable),
        PlayerCommand::PlayStation { station_id } => stations
            .iter()
            .position(|station| station.id == *station_id)
            .map(RemoteCommandPlan::PlayStation)
            .ok_or_else(unavailable),
        PlayerCommand::PlayStream {
            station_id,
            stream_uri,
        } => Ok(RemoteCommandPlan::PlayDelivered {
            station_id: station_id.clone(),
            stream_uri: stream_uri.clone(),
        }),
        PlayerCommand::SetVolume { level } => Ok(RemoteCommandPlan::SetVolume(*level)),
        PlayerCommand::ChangeVolume { delta } => Ok(RemoteCommandPlan::SetVolume(
            (i16::from(volume) + i16::from(*delta)).clamp(0, 100) as u8,
        )),
        PlayerCommand::Pause | PlayerCommand::SetMute { .. } => Err(CommandResult::failed(
            "capability_not_supported",
            "Command is not supported by this player",
        )),
        PlayerCommand::ChromecastDiscover => Ok(RemoteCommandPlan::ChromecastDiscover),
        PlayerCommand::ChromecastConnect { receiver_id } => {
            Ok(RemoteCommandPlan::ChromecastConnect {
                receiver_id: receiver_id.clone(),
            })
        }
        PlayerCommand::ChromecastDisconnect => Ok(RemoteCommandPlan::ChromecastDisconnect),
        PlayerCommand::RelayStart => Ok(RemoteCommandPlan::RelayStart),
        PlayerCommand::RelayStop => Ok(RemoteCommandPlan::RelayStop),
        PlayerCommand::RelaySetMode { mode } if mode == "via_pc" => {
            Ok(RemoteCommandPlan::RelayStart)
        }
        PlayerCommand::RelaySetMode { .. } => Err(CommandResult::failed(
            "capability_not_supported",
            "Relay mode is not supported by this player",
        )),
    }
}

impl RockCastApp {
    pub(in crate::app) fn queue_volume(&self) {
        let is_local = self
            .selected_device
            .and_then(|i| self.devices.get(i))
            .is_some_and(|d| d.is_local())
            || self.playing_local;
        self.playback.set_volume(is_local, self.volume);
    }

    pub(in crate::app) fn shutdown_playback(&mut self) {
        if self.shutting_down {
            return;
        }
        let generation = self.playback.current_generation() + 1;
        log::info!(
            "shutdown_playback: bump generation→{generation} playing={} local={}",
            self.playing,
            self.playing_local
        );
        self.shutting_down = true;
        self.device_control.shutdown();
        self.observers.stop();
        self.playing = false;
        self.playing_local = false;
        self.playing_url = None;
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);
        self.settings_writer.flush();
        // Local and relay stop are non-blocking. Process exit is the final
        // boundary for a Cast/network worker that is inside an OS call.
        self.playback.shutdown();
        log::info!("shutdown_playback: finished");
    }

    pub(in crate::app) fn play(&mut self) -> Option<u64> {
        if !self.can_start_play() {
            log::debug!(
                "play blocked: loading_devices={} devices={} selected_device={:?} selected_station={:?}",
                self.loading_devices,
                self.devices.len(),
                self.selected_device,
                self.selected_station
            );
            return None;
        }
        let Some(station) = self
            .selected_station
            .and_then(|index| self.stations.get(index))
            .cloned()
        else {
            self.status = self.lang.t().pick_station.into();
            return None;
        };
        let Some(device) = self
            .selected_device
            .and_then(|index| self.devices.get(index))
            .cloned()
        else {
            self.status = self.lang.t().pick_device.into();
            return None;
        };
        let local = device.is_local();
        // Bind the exact catalog station ID to this start lifecycle so the
        // published buffering/playing/error/stopped states all carry the
        // station that was actually chosen (live-control §4.5); the ID is
        // never recovered from the stream URL.
        self.playback_station_id = Some(station.id.clone());
        self.observers.stop();
        self.playing_url = None;
        self.playing_op = true;
        self.playing = false;
        self.playing_local = local;
        self.status = format!("Play: {} -> {}", station.name, device.name());
        self.station_now = station.name.clone();
        self.track = self.lang.t().connecting.into();
        self.mark_settings_dirty();
        self.persist_settings_if_needed(true);
        Some(self.playback.play(
            station,
            device,
            self.volume,
            self.cast_relay && !local,
            self.eq_enabled,
        ))
    }

    pub(in crate::app) fn stop(&mut self) -> Option<u64> {
        if self.shutting_down {
            return None;
        }
        self.playing_op = true;
        self.status = "Stop…".into();
        self.pending_voice_play = false;
        self.voice_fallback.clear();
        self.observers.stop();
        self.playing = false;
        self.playing_local = false;
        self.playing_url = None;
        self.track = self.lang.t().stopped.into();
        Some(self.playback.stop())
    }

    pub(in crate::app) fn poll_device_control_commands(&mut self) -> bool {
        for _ in 0..MAX_REMOTE_COMMANDS_PER_FRAME {
            let Some(command) = self.device_control.take_command() else {
                break;
            };
            let command_id = command.id;
            let plan = match plan_remote_command(
                &command.command,
                &self.stations,
                self.selected_station,
                self.volume,
            ) {
                Ok(plan) => plan,
                Err(result) => {
                    self.device_control.complete_command(&command_id, result);
                    continue;
                }
            };
            let generation = match plan {
                RemoteCommandPlan::PlaySelected => self.play(),
                RemoteCommandPlan::Stop => self.stop(),
                RemoteCommandPlan::PlayRelative(offset) => self.play_remote_relative(offset),
                RemoteCommandPlan::PlayStation(index) => {
                    self.selected_station = Some(index);
                    self.scroll_to_station = Some(index);
                    self.voice_fallback.clear();
                    self.play()
                }
                RemoteCommandPlan::PlayDelivered {
                    station_id,
                    stream_uri,
                } => {
                    // The server already validated this catalog station and resolved
                    // its stream (RS-3), so play it directly. A local id match only
                    // reuses metadata; the delivered URI stays authoritative.
                    let index = self.stations.iter().position(|s| s.id == station_id);
                    let index = match index {
                        Some(index) => {
                            if let Some(primary) = self.stations[index]
                                .streams
                                .iter_mut()
                                .find(|stream| stream.primary)
                            {
                                primary.url = stream_uri.clone();
                            }
                            self.stations[index].url = stream_uri;
                            index
                        }
                        None => {
                            self.stations
                                .push(delivered_station(&station_id, &stream_uri));
                            self.stations.len() - 1
                        }
                    };
                    self.selected_station = Some(index);
                    self.scroll_to_station = Some(index);
                    self.voice_fallback.clear();
                    self.play()
                }
                RemoteCommandPlan::SetVolume(level) => {
                    self.volume = level;
                    self.queue_volume();
                    self.mark_settings_dirty();
                    self.persist_settings_if_needed(true);
                    self.device_control
                        .complete_command(&command_id, CommandResult::succeeded());
                    continue;
                }
                RemoteCommandPlan::ChromecastDiscover => {
                    self.discover_chromecasts(&command_id);
                    continue;
                }
                RemoteCommandPlan::ChromecastConnect { receiver_id } => {
                    let Some(device) = self
                        .chromecast_receivers
                        .get_fresh_at(&receiver_id, OffsetDateTime::now_utc())
                    else {
                        self.device_control.complete_command(
                            &command_id,
                            CommandResult::failed(
                                "invalid_payload",
                                "Chromecast receiver is missing or discovery has expired",
                            ),
                        );
                        continue;
                    };
                    if self.selected_station.is_none() {
                        self.device_control.complete_command(
                            &command_id,
                            CommandResult::failed(
                                "invalid_payload",
                                "A selected station is required before connecting Chromecast",
                            ),
                        );
                        continue;
                    }
                    self.select_cast_device(device);
                    self.cast_relay = false;
                    let generation = self.play();
                    self.track_remote_transition(
                        &command_id,
                        generation,
                        super::super::RemoteOutput::Chromecast(Some(receiver_id)),
                    );
                    continue;
                }
                RemoteCommandPlan::ChromecastDisconnect => {
                    if matches!(&self.output, super::super::RemoteOutput::Local) {
                        self.device_control
                            .complete_command(&command_id, CommandResult::succeeded());
                        continue;
                    }
                    let Some(local_index) = self.devices.iter().position(OutputDevice::is_local)
                    else {
                        self.device_control.complete_command(
                            &command_id,
                            CommandResult::failed(
                                "invalid_payload",
                                "No local output is available for Chromecast fallback",
                            ),
                        );
                        continue;
                    };
                    if self.selected_station.is_none() {
                        self.device_control.complete_command(
                            &command_id,
                            CommandResult::failed(
                                "invalid_payload",
                                "A selected station is required before local fallback",
                            ),
                        );
                        continue;
                    }
                    self.selected_device = Some(local_index);
                    self.cast_relay = false;
                    let generation = self.play();
                    self.track_remote_transition(
                        &command_id,
                        generation,
                        super::super::RemoteOutput::Local,
                    );
                    continue;
                }
                RemoteCommandPlan::RelayStart => {
                    if matches!(&self.output, super::super::RemoteOutput::Relay(_))
                        && self.playback.relay_active()
                    {
                        self.device_control
                            .complete_command(&command_id, CommandResult::succeeded());
                        continue;
                    }
                    let receiver_id = match &self.output {
                        super::super::RemoteOutput::Chromecast(receiver_id) => receiver_id.clone(),
                        _ => {
                            self.device_control.complete_command(
                                &command_id,
                                CommandResult::failed(
                                    "invalid_payload",
                                    "Relay requires confirmed Chromecast playback",
                                ),
                            );
                            continue;
                        }
                    };
                    if !self.playing
                        || self.selected_station.is_none()
                        || self
                            .selected_device
                            .and_then(|index| self.devices.get(index))
                            .is_none_or(|device| device.is_local())
                    {
                        self.device_control.complete_command(
                            &command_id,
                            CommandResult::failed(
                                "invalid_payload",
                                "Relay requires active Chromecast playback",
                            ),
                        );
                        continue;
                    }
                    self.cast_relay = true;
                    let generation = self.play();
                    self.track_remote_transition(
                        &command_id,
                        generation,
                        super::super::RemoteOutput::Relay(receiver_id),
                    );
                    continue;
                }
                RemoteCommandPlan::RelayStop => {
                    let receiver_id = match &self.output {
                        super::super::RemoteOutput::Relay(receiver_id) => receiver_id.clone(),
                        super::super::RemoteOutput::Chromecast(_) => {
                            self.device_control
                                .complete_command(&command_id, CommandResult::succeeded());
                            continue;
                        }
                        _ => {
                            self.device_control.complete_command(
                                &command_id,
                                CommandResult::failed("invalid_payload", "Relay is not active"),
                            );
                            continue;
                        }
                    };
                    if !self.playing || self.selected_station.is_none() {
                        self.device_control.complete_command(
                            &command_id,
                            CommandResult::failed("invalid_payload", "Relay output is unavailable"),
                        );
                        continue;
                    }
                    self.cast_relay = false;
                    let generation = self.play();
                    self.track_remote_transition(
                        &command_id,
                        generation,
                        super::super::RemoteOutput::Chromecast(receiver_id),
                    );
                    continue;
                }
            };
            if let Some(generation) = generation {
                if let Some(previous) =
                    self.pending_remote_command
                        .replace(super::super::PendingRemoteCommand {
                            id: command_id,
                            generation,
                            output: None,
                        })
                {
                    self.device_control.complete_command(
                        &previous.id,
                        CommandResult::failed("command_timeout", "Command was interrupted"),
                    );
                }
            } else {
                self.device_control.complete_command(
                    &command_id,
                    CommandResult::failed(
                        "invalid_payload",
                        "Command cannot be applied to local playback state",
                    ),
                );
            }
        }
        self.device_control.has_queued_commands()
    }

    fn discover_chromecasts(&mut self, command_id: &str) {
        if self.pending_chromecast_discovery.is_some() {
            self.device_control.complete_command(
                command_id,
                CommandResult::failed("command_timeout", "Chromecast discovery is already running"),
            );
            return;
        }
        let command_id = command_id.to_string();
        let completion_id = command_id.clone();
        self.pending_chromecast_discovery = Some(command_id.clone());
        let tx = self.ui_tx.clone();
        if self
            .background
            .spawn(move |_| {
                let result = LocalChromecastDiscovery.discover(Duration::from_secs(5));
                let _ = tx.send(super::super::messages::UiMsg::RemoteChromecastDiscovery {
                    command_id,
                    result,
                });
            })
            .is_err()
        {
            self.pending_chromecast_discovery = None;
            self.device_control.complete_command(
                &completion_id,
                CommandResult::failed("command_timeout", "Chromecast discovery could not start"),
            );
        }
    }

    fn select_cast_device(&mut self, device: crate::cast::CastDeviceInfo) {
        let device = OutputDevice::Cast(device);
        let id = device.id().to_string();
        if let Some(index) = self
            .devices
            .iter()
            .position(|existing| super::super::messages::same_output_device(existing, &device))
        {
            self.devices[index] = device;
            self.selected_device = Some(index);
        } else {
            self.devices.push(device);
            self.devices.sort_by_key(|device| !device.is_local());
            self.selected_device = self.devices.iter().position(|device| device.id() == id);
        }
    }

    fn track_remote_transition(
        &mut self,
        command_id: &str,
        generation: Option<u64>,
        output: super::super::RemoteOutput,
    ) {
        let Some(generation) = generation else {
            self.device_control.complete_command(
                command_id,
                CommandResult::failed(
                    "invalid_payload",
                    "Command cannot be applied to local playback state",
                ),
            );
            return;
        };
        if let Some(previous) =
            self.pending_remote_command
                .replace(super::super::PendingRemoteCommand {
                    id: command_id.to_string(),
                    generation,
                    output: Some(output),
                })
        {
            self.device_control.complete_command(
                &previous.id,
                CommandResult::failed("command_timeout", "Command was interrupted"),
            );
        }
    }

    fn play_remote_relative(&mut self, offset: isize) -> Option<u64> {
        let current = self.selected_station?;
        let next = current
            .checked_add_signed(offset)
            .filter(|next| *next < self.stations.len())?;
        self.selected_station = Some(next);
        self.scroll_to_station = Some(next);
        self.voice_fallback.clear();
        self.play()
    }

    pub(in crate::app) fn apply_voice_control(&mut self, control: VoiceControl) {
        match control {
            VoiceControl::PlayLast => self.play_last_station(),
            VoiceControl::Stop => {
                log::info!("voice control: stop");
                self.stop();
            }
            VoiceControl::Next => self.play_relative_station(1, "next"),
            VoiceControl::Previous => self.play_relative_station(-1, "previous"),
        }
    }

    fn play_last_station(&mut self) {
        if let Some(station_id) = self
            .personal_data
            .as_ref()
            .and_then(|store| store.last_played_station_id())
            && let Some(index) = self
                .stations
                .iter()
                .position(|station| station.id == station_id)
        {
            self.selected_station = Some(index);
            self.scroll_to_station = Some(index);
            self.play();
            return;
        }
        let Some(station) = self.last_played_station.clone() else {
            self.status = "Voice play music: no previously played station".into();
            return;
        };
        let index = self
            .stations
            .iter()
            .position(|candidate| candidate.url == station.url)
            .unwrap_or_else(|| {
                self.stations.insert(0, station.clone());
                0
            });
        self.selected_station = Some(index);
        self.scroll_to_station = Some(index);
        self.voice_fallback.clear();
        log::info!(
            "voice control: play last station idx={index} name={:?} url={}",
            station.name,
            station.url
        );
        self.play();
    }

    fn play_relative_station(&mut self, offset: isize, command: &str) {
        let Some(current) = self.selected_station else {
            self.status = self.lang.t().pick_station.into();
            return;
        };
        let Some(next) = current.checked_add_signed(offset) else {
            self.status = format!("Voice {command}: no station in that direction");
            return;
        };
        if next >= self.stations.len() {
            self.status = format!("Voice {command}: no station in that direction");
            return;
        }

        self.selected_station = Some(next);
        self.scroll_to_station = Some(next);
        self.voice_fallback.clear();
        if let Some(station) = self.stations.get(next) {
            log::info!(
                "voice control: {command} station idx={next} name={:?} url={}",
                station.name,
                station.url
            );
        }
        self.play();
    }

    pub(in crate::app) fn schedule_stream_tap(&mut self, generation: u64, tap_url: String) {
        self.observers
            .schedule(generation, tap_url, self.playback.relay_active());
    }

    pub(in crate::app) fn sync_spectrum(&mut self) {
        if self.playback.relay_active() {
            self.observers.stop();
            return;
        }
        let tap = self.playing_url.clone();
        let relay_url = self.playback.relay_public_url();
        self.observers.sync(
            self.playing,
            self.playing_local,
            self.eq_enabled,
            tap,
            relay_url.as_deref(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(id: &str, url: &str) -> crate::stations::Station {
        crate::stations::Station::from_primary(
            id.into(),
            id.into(),
            url.into(),
            String::new(),
            String::new(),
            128,
            "mp3".into(),
        )
    }

    #[derive(Default)]
    struct FakePlayback {
        calls: Vec<RemoteCommandPlan>,
    }

    impl FakePlayback {
        fn apply(&mut self, plan: RemoteCommandPlan) {
            self.calls.push(plan);
        }
    }

    #[test]
    fn remote_commands_map_once_to_existing_playback_operations() {
        let stations = [
            station("first", "https://catalog.test/first"),
            station("second", "https://catalog.test/second"),
        ];
        let commands = [
            (PlayerCommand::Play, RemoteCommandPlan::PlaySelected),
            (PlayerCommand::Stop, RemoteCommandPlan::Stop),
            (PlayerCommand::Next, RemoteCommandPlan::PlayRelative(1)),
            (PlayerCommand::Previous, RemoteCommandPlan::PlayRelative(-1)),
            (
                PlayerCommand::PlayStation {
                    station_id: "second".into(),
                },
                RemoteCommandPlan::PlayStation(1),
            ),
            (
                PlayerCommand::PlayStream {
                    station_id: "first".into(),
                    stream_uri: "https://catalog.test/first".into(),
                },
                RemoteCommandPlan::PlayDelivered {
                    station_id: "first".into(),
                    stream_uri: "https://catalog.test/first".into(),
                },
            ),
            (
                PlayerCommand::SetVolume { level: 77 },
                RemoteCommandPlan::SetVolume(77),
            ),
            (
                PlayerCommand::ChangeVolume { delta: 20 },
                RemoteCommandPlan::SetVolume(70),
            ),
        ];
        let mut playback = FakePlayback::default();
        for (command, expected) in commands {
            let selected = if matches!(&command, PlayerCommand::Previous) {
                Some(1)
            } else {
                Some(0)
            };
            let plan = plan_remote_command(&command, &stations, selected, 50).unwrap();
            assert_eq!(plan, expected);
            playback.apply(plan);
        }
        assert_eq!(playback.calls.len(), 8);
    }

    #[test]
    fn remote_command_failures_do_not_create_a_playback_operation() {
        let stations = [station("known", "https://catalog.test/known")];
        for command in [
            PlayerCommand::Next,
            PlayerCommand::Previous,
            PlayerCommand::PlayStation {
                station_id: "missing".into(),
            },
            PlayerCommand::Pause,
            PlayerCommand::SetMute { muted: true },
        ] {
            assert!(plan_remote_command(&command, &stations, None, 50).is_err());
        }
    }

    #[test]
    fn delivered_server_stream_plays_without_local_catalog_membership() {
        // The phone's catalog stations are not part of RockCast's local list; the
        // server-resolved command must still plan a direct playback start.
        let stations = [station("known", "https://catalog.test/known")];
        let command = PlayerCommand::PlayStream {
            station_id: "server-station".into(),
            stream_uri: "https://stream.test/server".into(),
        };
        let plan = plan_remote_command(&command, &stations, None, 50).unwrap();
        assert_eq!(
            plan,
            RemoteCommandPlan::PlayDelivered {
                station_id: "server-station".into(),
                stream_uri: "https://stream.test/server".into(),
            }
        );
    }
}
