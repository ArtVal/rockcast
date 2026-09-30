//! Voice recognition result message handling.

use crate::{
    app::{RockCastApp, StationFilterMode},
    voice::{VoiceCommandStatus, VoiceError, VoiceOutcome, VoiceStreamErrorCode},
};

impl RockCastApp {
    pub(super) fn handle_voice_result(&mut self, result: Result<VoiceOutcome, VoiceError>) {
        self.voice_busy = false;
        self.voice_recording = None;
        match result {
            Ok(VoiceOutcome::DeviceCommand { status, .. }) => match status {
                VoiceCommandStatus::Succeeded => {
                    self.status = "Голосовая команда исполнена".into();
                }
                VoiceCommandStatus::Failed => {
                    self.status = "Голосовое управление: команда не выполнена".into();
                }
            },
            Ok(VoiceOutcome::Legacy(result)) => {
                if let Some(control) = result.control {
                    self.apply_voice_control(control);
                    return;
                }
                let stations = result.stations;
                log::info!("voice candidates received: count={}", stations.len());
                let first = stations[0].clone();
                self.voice_fallback = stations.iter().skip(1).cloned().collect();
                self.station_now = first.name.clone();
                self.station_request_id = self.station_request_id.wrapping_add(1);
                self.stations = stations;
                self.loading_stations = false;
                self.loading_more_stations = false;
                self.loading_more_error = None;
                self.station_search_total = Some(self.stations.len());
                self.station_search_offset = self.stations.len();
                self.station_has_more = false;
                self.filter_mode = StationFilterMode::All;
                self.selected_genre = None;
                self.selected_country = None;
                self.selected_min_bitrate = None;
                let clean_query = crate::voice::clean_voice_query(&result.transcript);
                self.voice_search_query = if !clean_query.is_empty() {
                    Some(clean_query.clone())
                } else if !result.transcript.trim().is_empty() {
                    Some(result.transcript.trim().to_string())
                } else {
                    None
                };
                self.station_search.clear();
                self.queue_station_icons(&self.stations.clone());
                self.source = format!("RockServer · голос · {}", self.stations.len());
                self.selected_station = Some(0);
                self.scroll_to_station = Some(0);
                log::info!(
                    "voice selected first station: name={:?} url={} fallbacks={}",
                    self.stations[0].name,
                    self.stations[0].url,
                    self.voice_fallback.len()
                );
                if result.auto_play {
                    crate::voice_prompts::play(
                        crate::voice_prompts::Prompt::TurningOn,
                        self.lang,
                    );
                    self.status = "Голосовая команда распознана; запускаю станцию".into();
                    if self.can_start_play() {
                        self.play();
                    } else {
                        self.pending_voice_play = true;
                        self.status = "Команда распознана; ожидаю аудиоустройство…".into();
                    }
                } else {
                    self.pending_voice_play = false;
                    self.voice_fallback.clear();
                    self.status = format!(
                        "Найдено станций: {}. Список отсортирован по похожести.",
                        self.stations.len()
                    );
                }

                if !clean_query.is_empty() {
                    self.search_stations(clean_query);
                }
            }
            Err(error) => {
                let prompt = match &error {
                    VoiceError::ServerUnavailable => {
                        Some(crate::voice_prompts::Prompt::ServerUnavailable)
                    }
                    VoiceError::TokenMissing => Some(crate::voice_prompts::Prompt::TokenMissing),
                    VoiceError::TokenInvalid => Some(crate::voice_prompts::Prompt::TokenInvalid),
                    VoiceError::NotFound => Some(crate::voice_prompts::Prompt::NotFound),
                    VoiceError::StreamError {
                        code: VoiceStreamErrorCode::StationNotFound,
                        ..
                    } => Some(crate::voice_prompts::Prompt::NotFound),
                    _ => None,
                };
                if let Some(prompt) = prompt {
                    crate::voice_prompts::play(prompt, self.lang);
                }
                self.status = format!("Голосовое управление: {error}");
            }
        }
    }
}
