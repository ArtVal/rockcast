//! Playback event handling and remote command confirmation.

use crate::app::RockCastApp;
use crate::playback::PlaybackEvent;

fn is_station_unavailable_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "404",
        "station unavailable",
        "failed to open audio stream",
        "upstream",
        "timed out",
        "timeout",
        "eof",
        "stalled in idle",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

impl RockCastApp {
    pub(super) fn handle_playback_event(&mut self, event: PlaybackEvent) {
        match event {
            PlaybackEvent::Status { text, .. } => self.status = text,
            PlaybackEvent::Title { title, .. } => {
                self.track_metadata = crate::app::bounded_track_title(&title, &self.station_now);
                self.track = title;
            }
            PlaybackEvent::PlayOk {
                url,
                tap_url,
                generation,
                local,
            } => {
                self.playing_op = false;
                self.playing = true;
                self.playing_local = local;
                self.playing_url = Some(url.clone());
                self.confirm_playback_output(generation, local);
                if let Some(station) = self
                    .stations
                    .iter()
                    .find(|station| station.url == url)
                    .cloned()
                {
                    self.last_played_station = Some(station.clone());
                    if let Some(store) = self.personal_data.as_mut()
                        && let Err(error) = store.record_play(&station)
                    {
                        log::warn!("failed to record playback history: {error}");
                    }
                    self.schedule_personal_sync();
                }
                // The stream often delivers its first ICY title while the
                // playout buffer is still filling, i.e. before PlayOk —
                // only show the hint when no real title has arrived yet.
                let placeholder = self.track.is_empty()
                    || [
                        self.lang.t().track_hint,
                        self.lang.t().track_meta_hint,
                        self.lang.t().stopped,
                        self.lang.t().connecting,
                    ]
                    .contains(&self.track.as_str());
                if placeholder {
                    self.track = self.lang.t().track_meta_hint.into();
                }
                if !local
                    && !self.playback.relay_active()
                    && self.eq_enabled
                    && let Some(tap_url) = tap_url
                {
                    self.schedule_stream_tap(generation, tap_url);
                }
                self.finish_remote_command(generation, true);
            }
            PlaybackEvent::StopOk { generation } => {
                self.playing_op = false;
                self.playing = false;
                self.playing_local = false;
                self.playing_url = None;
                self.observers.stop();
                self.track = self.lang.t().stopped.into();
                self.track_metadata = None;
                self.status = self.lang.t().stopped.into();
                self.output = crate::app::RemoteOutput::Local;
                self.finish_remote_command(generation, true);
            }
            PlaybackEvent::Error {
                message,
                generation,
            } => {
                if is_station_unavailable_error(&message) {
                    crate::voice_prompts::play(
                        crate::voice_prompts::Prompt::StationUnavailable,
                        self.lang,
                    );
                }
                self.playing_op = false;
                self.playing = false;
                self.playing_local = false;
                self.playing_url = None;
                self.observers.stop();
                let failed_url = self
                    .selected_station
                    .and_then(|index| self.stations.get(index))
                    .map(|station| station.url.clone());
                if let Some(failed_url) = failed_url {
                    self.stations.retain(|station| station.url != failed_url);
                    log::warn!("removing unavailable station: {failed_url}: {message}");
                }
                self.selected_station = None;
                self.station_now = "—".into();
                self.track = self.lang.t().track_hint.into();
                self.track_metadata = None;
                self.output = crate::app::RemoteOutput::Local;
                if let Some(next) = self.voice_fallback.pop_front() {
                    log::info!(
                        "voice fallback: trying next station name={:?} url={} remaining={}",
                        next.name,
                        next.url,
                        self.voice_fallback.len()
                    );
                    self.status =
                        format!("Станция недоступна; пробую следующую: {}", next.name);
                    self.stations.retain(|station| station.url != next.url);
                    self.stations.insert(0, next);
                    self.selected_station = Some(0);
                    self.scroll_to_station = Some(0);
                    self.play();
                } else {
                    self.status = message;
                }
                self.finish_remote_command(generation, false);
            }
        }
    }

    pub(super) fn finish_remote_command(&mut self, generation: u64, succeeded: bool) {
        let Some(pending) = self.pending_remote_command.take() else {
            return;
        };
        if pending.generation != generation {
            self.pending_remote_command = Some(pending);
            return;
        }
        if succeeded && let Some(output) = pending.output {
            self.output = output;
        }
        self.device_control.complete_command(
            &pending.id,
            if succeeded {
                crate::device_control::CommandResult::succeeded()
            } else {
                // The v1 error enum lacks an execution_failed value.  A failed
                // command is still terminal; no optimistic state is published.
                crate::device_control::CommandResult::failed(
                    "command_timeout",
                    "Playback failed or was interrupted",
                )
            },
        );
    }

    pub(super) fn confirm_playback_output(&mut self, generation: u64, local: bool) {
        if local {
            self.output = crate::app::RemoteOutput::Local;
            return;
        }
        let pending = self
            .pending_remote_command
            .as_ref()
            .filter(|pending| pending.generation == generation)
            .and_then(|pending| pending.output.clone());
        self.output = pending.unwrap_or_else(|| {
            if self.playback.relay_active() {
                crate::app::RemoteOutput::Relay(None)
            } else {
                crate::app::RemoteOutput::Chromecast(None)
            }
        });
    }

    pub(super) fn finish_stale_remote_command(&mut self) {
        if self
            .pending_remote_command
            .as_ref()
            .is_some_and(|pending| pending.generation != self.playback.current_generation())
            && let Some(pending) = self.pending_remote_command.take()
        {
            self.device_control.complete_command(
                &pending.id,
                crate::device_control::CommandResult::failed(
                    "command_timeout",
                    "Command was interrupted",
                ),
            );
        }
    }
}
