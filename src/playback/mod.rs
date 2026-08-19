//! Playback orchestration independent from egui.

mod phase;
mod volume;

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

use crate::{
    cast::CastService,
    local::LocalPlayer,
    output::OutputDevice,
    relay::StreamRelay,
    runtime::BackgroundRuntime,
    stations::Station,
};

pub use phase::{PlaybackEvent, PlaybackPhase};
use volume::{cast_volume, local_volume};

pub struct PlaybackController {
    cast: Arc<CastService>,
    local: Arc<LocalPlayer>,
    relay: Arc<StreamRelay>,
    runtime: BackgroundRuntime,
    generation: Arc<AtomicU64>,
    phase: PlaybackPhase,
    event_tx: mpsc::Sender<PlaybackEvent>,
    event_rx: mpsc::Receiver<PlaybackEvent>,
}

impl PlaybackController {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            cast: Arc::new(CastService::new()),
            local: Arc::new(LocalPlayer::new()),
            relay: Arc::new(StreamRelay::new()),
            runtime: BackgroundRuntime::new(6),
            generation: Arc::new(AtomicU64::new(0)),
            phase: PlaybackPhase::Idle,
            event_tx,
            event_rx,
        }
    }

    pub fn phase(&self) -> PlaybackPhase {
        self.phase
    }

    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.current_generation() == generation
    }

    pub fn try_event(&self) -> Option<PlaybackEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn relay_public_url(&self) -> Option<String> {
        self.relay.public_url()
    }

    pub fn relay_latest_title(&self) -> Option<String> {
        self.relay.latest_title()
    }

    pub fn relay_tap_url(&self) -> Option<String> {
        self.relay.tap_url()
    }

    pub fn relay_levels(&self) -> [f32; crate::audio::spectrum::BANDS] {
        self.relay.levels()
    }

    pub fn relay_active(&self) -> bool {
        self.relay.is_active()
    }

    pub fn local_levels(&self) -> [f32; crate::audio::spectrum::BANDS] {
        self.local.levels()
    }

    pub fn play(
        &mut self,
        station: Station,
        device: OutputDevice,
        volume: u8,
        use_relay: bool,
        spectrum_enabled: bool,
    ) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.cast.cancel_pending();
        let local_output = device.is_local();
        self.phase = PlaybackPhase::Opening {
            generation,
            local: local_output,
        };

        let tx = self.event_tx.clone();
        let cast = Arc::clone(&self.cast);
        let local = Arc::clone(&self.local);
        let relay = Arc::clone(&self.relay);
        let play_generation = Arc::clone(&self.generation);
        let (title_tx, title_rx) = mpsc::channel();
        if local_output {
            let title_events = self.event_tx.clone();
            let title_generation = Arc::clone(&self.generation);
            let _ = self.runtime.spawn(move |cancel| {
                while !cancel.is_cancelled()
                    && title_generation.load(Ordering::Acquire) == generation
                {
                    match title_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                        Ok(title) if title_generation.load(Ordering::Acquire) == generation => {
                            let _ = title_events.send(PlaybackEvent::Title { title, generation });
                        }
                        Ok(_) => {}
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        }
        let submit = self.runtime.spawn(move |runtime_cancel| match device {
            OutputDevice::Cast(cast_dev) => {
                local.stop();
                relay.stop();
                if runtime_cancel.is_cancelled()
                    || play_generation.load(Ordering::Acquire) != generation
                {
                    return;
                }

                let cancel = std::sync::atomic::AtomicBool::new(false);
                let mut relay_owned = false;
                let (load_url, load_ct) = if use_relay {
                    match relay.start(
                        &station.url,
                        &cast_dev.discovered.host,
                        station.content_type(),
                        &cancel,
                    ) {
                        Ok(value) => {
                            relay_owned = true;
                            value
                        }
                        Err(e) => {
                            let _ = tx.send(PlaybackEvent::Error {
                                message: e.to_string(),
                                generation,
                            });
                            return;
                        }
                    }
                } else {
                    (station.url.clone(), station.content_type().to_string())
                };

                if use_relay {
                    let min_pcm = 48_000usize * 2 * 2;
                    let min_bytes = if load_ct.contains("wav") {
                        min_pcm
                    } else {
                        8 * 1024
                    };
                    let format_timeout = std::time::Duration::from_secs(12);
                    let buffer_timeout = std::time::Duration::from_secs(12);
                    if load_ct.contains("wav") && !relay.wait_for_pcm_format(format_timeout) {
                        log::warn!("relay pre-buffer: PCM format not ready within {format_timeout:?}");
                    }
                    if !relay.wait_for_data(min_bytes, buffer_timeout) {
                        log::warn!(
                            "relay pre-buffer short: wanted {min_bytes} B within {buffer_timeout:?}"
                        );
                    }
                }

                if runtime_cancel.is_cancelled()
                    || play_generation.load(Ordering::Acquire) != generation
                {
                    if relay_owned {
                        relay.stop();
                    }
                    return;
                }
                let result = cast.play(&cast_dev, &load_url, &load_ct, &station.name, |status| {
                    if play_generation.load(Ordering::Acquire) == generation {
                        let _ = tx.send(PlaybackEvent::Status {
                            text: status.to_string(),
                            generation,
                        });
                    }
                });
                if play_generation.load(Ordering::Acquire) != generation {
                    return;
                }
                match result {
                    Ok(()) => {
                        let _ = cast.set_volume_current(cast_volume(volume));
                        let tap_url = if relay_owned {
                            relay.tap_url()
                        } else {
                            Some(station.url.clone())
                        };
                        let _ = tx.send(PlaybackEvent::PlayOk {
                            url: station.url,
                            tap_url,
                            generation,
                            local: false,
                        });
                    }
                    Err(e) => {
                        if relay_owned {
                            relay.stop();
                        }
                        let _ = tx.send(PlaybackEvent::Error {
                            message: e.to_string(),
                            generation,
                        });
                    }
                }
            }
            OutputDevice::Local(local_dev) => {
                relay.stop();
                let _ = cast.stop();
                if runtime_cancel.is_cancelled()
                    || play_generation.load(Ordering::Acquire) != generation
                {
                    return;
                }
                let result = local.play(
                    &local_dev,
                    &station.url,
                    local_volume(volume),
                    spectrum_enabled,
                    Some(title_tx),
                    |status| {
                        if play_generation.load(Ordering::Acquire) == generation {
                            let _ = tx.send(PlaybackEvent::Status {
                                text: status.to_string(),
                                generation,
                            });
                        }
                    },
                );
                if play_generation.load(Ordering::Acquire) != generation {
                    // Never stop here: a newer local session may already own LocalPlayer.
                    return;
                }
                match result {
                    Ok(()) => {
                        let _ = tx.send(PlaybackEvent::PlayOk {
                            url: station.url.clone(),
                            tap_url: Some(station.url),
                            generation,
                            local: true,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(PlaybackEvent::Error {
                            message: e.to_string(),
                            generation,
                        });
                    }
                }
            }
        });

        if let Err(e) = submit {
            self.phase = PlaybackPhase::Failed { generation };
            let _ = self.event_tx.send(PlaybackEvent::Error {
                message: e.into(),
                generation,
            });
        }
        generation
    }

    pub fn stop(&mut self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.phase = PlaybackPhase::Stopping { generation };
        let tx = self.event_tx.clone();
        let cast = Arc::clone(&self.cast);
        let local = Arc::clone(&self.local);
        let relay = Arc::clone(&self.relay);
        if let Err(e) = self.runtime.spawn(move |_| {
            local.stop();
            relay.stop();
            match cast.stop() {
                Ok(()) => {
                    let _ = tx.send(PlaybackEvent::StopOk { generation });
                }
                Err(e) => {
                    let _ = tx.send(PlaybackEvent::Error {
                        message: e.to_string(),
                        generation,
                    });
                }
            }
        }) {
            let _ = self.event_tx.send(PlaybackEvent::Error {
                message: e.into(),
                generation,
            });
        }
        generation
    }

    pub fn set_volume(&self, local_output: bool, percent: u8) {
        let cast = Arc::clone(&self.cast);
        let local = Arc::clone(&self.local);
        let _ = self.runtime.spawn(move |_| {
            if local_output {
                local.set_volume(local_volume(percent));
            } else {
                let _ = cast.set_volume_current(cast_volume(percent));
            }
        });
    }

    pub fn spawn_job(
        &self,
        job: impl FnOnce(crate::runtime::CancelToken) + Send + 'static,
    ) -> Result<(), &'static str> {
        self.runtime.spawn(job)
    }

    pub fn apply_event(&mut self, event: &PlaybackEvent) -> bool {
        let generation = match event {
            PlaybackEvent::Status { generation, .. }
            | PlaybackEvent::Title { generation, .. }
            | PlaybackEvent::PlayOk { generation, .. }
            | PlaybackEvent::StopOk { generation }
            | PlaybackEvent::Error { generation, .. } => *generation,
        };
        if generation != self.current_generation() {
            return false;
        }
        self.phase = match event {
            PlaybackEvent::Title { .. } => return true,
            PlaybackEvent::PlayOk { local, .. } => PlaybackPhase::Playing {
                generation,
                local: *local,
            },
            PlaybackEvent::StopOk { .. } => PlaybackPhase::Idle,
            PlaybackEvent::Error { .. } => PlaybackPhase::Failed { generation },
            PlaybackEvent::Status { .. } => return true,
        };
        true
    }

    pub fn shutdown(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.local.stop();
        self.relay.stop();
        self.cast.cancel_pending();
        self.runtime.shutdown();
        self.phase = PlaybackPhase::Idle;
    }
}

impl Default for PlaybackController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn phase_exposes_generation() {
        assert_eq!(PlaybackPhase::Idle.generation(), None);
        assert_eq!(
            PlaybackPhase::Stopping { generation: 4 }.generation(),
            Some(4)
        );
    }

    #[test]
    fn stale_events_cannot_replace_current_state() {
        let mut controller = PlaybackController::new();
        controller.generation.store(9, Ordering::Release);
        controller.phase = PlaybackPhase::Opening {
            generation: 9,
            local: false,
        };
        let stale = PlaybackEvent::PlayOk {
            url: "old".into(),
            tap_url: Some("old".into()),
            generation: 8,
            local: true,
        };
        assert!(!controller.apply_event(&stale));
        assert_eq!(
            controller.phase(),
            PlaybackPhase::Opening {
                generation: 9,
                local: false
            }
        );
    }

    #[test]
    fn current_success_enters_playing() {
        let mut controller = PlaybackController::new();
        controller.generation.store(3, Ordering::Release);
        let event = PlaybackEvent::PlayOk {
            url: "station".into(),
            tap_url: Some("station".into()),
            generation: 3,
            local: true,
        };
        assert!(controller.apply_event(&event));
        assert_eq!(
            controller.phase(),
            PlaybackPhase::Playing {
                generation: 3,
                local: true
            }
        );
    }
}
