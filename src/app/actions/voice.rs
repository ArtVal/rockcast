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
        let _ = self.playback.spawn_job(move |_| {
            let _ = tx.send(UiMsg::VoiceResult(crate::voice::capture_and_recognize(
                rockserver.base_url(),
                rockserver.bearer_token(),
                &locale,
                rockserver.recognizer_mode(),
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
