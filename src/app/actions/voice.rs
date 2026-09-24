//! RockServer voice capture and recognition.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::super::{RockCastApp, messages::UiMsg};

impl RockCastApp {
    pub(in crate::app) fn start_voice(&mut self) {
        if self.voice_busy {
            return;
        }
        self.voice_busy = true;
        crate::voice_prompts::play(crate::voice_prompts::Prompt::Beep, self.lang);
        log::info!("voice button pressed: locale=ru-RU");
        self.status = "Слушаю, пока удерживается кнопка…".into();
        let recording = Arc::new(AtomicBool::new(true));
        self.voice_recording = Some(Arc::clone(&recording));
        let tx = self.ui_tx.clone();
        let rockserver = self.rockserver.clone();
        // Voice commands are currently Russian regardless of UI translation.
        let locale = "ru-RU".to_owned();
        if self
            .background
            .spawn(move |_| {
                let bearer_token = crate::session::AccountClient::new(
                    rockserver.clone(),
                    crate::session::OsCredentialStore,
                )
                .voice_access_token()
                .ok()
                .flatten();
                let effective_token = bearer_token.as_deref().or(rockserver.bearer_token());
                let _ = tx.send(UiMsg::VoiceResult(crate::voice::capture_and_recognize(
                    rockserver.base_url(),
                    effective_token,
                    &locale,
                    rockserver.recognizer_mode(),
                    None,
                    recording,
                )));
            })
            .is_err()
        {
            self.voice_busy = false;
            self.voice_recording = None;
            self.status = self.lang.t().background_busy.into();
        }
    }

    pub(in crate::app) fn stop_voice_recording(&mut self) {
        if let Some(recording) = self.voice_recording.take() {
            log::info!("voice button released: committing captured audio");
            recording.store(false, Ordering::Release);
            self.status = "Распознаю команду…".into();
        }
    }
}
