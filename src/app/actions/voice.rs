//! RockServer voice capture and recognition.

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use super::super::{messages::UiMsg, RockCastApp};

impl RockCastApp {
    pub(in crate::app) fn start_voice(&mut self) {
        if !self.rockserver_enabled || self.voice_busy {
            return;
        }
        if self.rockserver_bearer_token.trim().is_empty() {
            self.rockserver_setup_open = true;
            crate::voice_prompts::play(
                crate::voice_prompts::Prompt::TokenMissing,
                self.lang,
            );
            self.status = self.lang.t().rockserver_token_required.into();
            return;
        }
        self.voice_busy = true;
        crate::voice_prompts::play(crate::voice_prompts::Prompt::Beep, self.lang);
        log::info!(
            "voice button pressed: locale=ru-RU rockserver_url={}",
            self.rockserver_url
        );
        self.status = "Слушаю, пока удерживается кнопка…".into();
        let recording = Arc::new(AtomicBool::new(true));
        self.voice_recording = Some(Arc::clone(&recording));
        let tx = self.ui_tx.clone();
        let url = self.rockserver_url.clone();
        let bearer_token = self.rockserver_bearer_token.clone();
        // Voice commands are currently Russian regardless of UI translation.
        let locale = "ru-RU".to_owned();
        let _ = self.playback.spawn_job(move |_| {
            let _ = tx.send(UiMsg::VoiceResult(crate::voice::capture_and_recognize(
                &url,
                &bearer_token,
                &locale,
                recording,
            )));
        });
    }

    pub(in crate::app) fn stop_voice_recording(&mut self) {
        if let Some(recording) = self.voice_recording.take() {
            log::info!("voice button released: committing captured audio");
            recording.store(false, Ordering::Release);
            self.status = "Распознаю команду…".into();
        }
    }
}
